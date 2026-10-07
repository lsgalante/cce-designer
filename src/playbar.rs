//! The playbar pane: a full-width animation-transport strip along the
//! window's bottom edge, below the other panes (`PLAYBAR_IDX`). App-owned on the narrow traits wrapped in
//! `Adapted<Playbar>`, like `Viewport3D`. Unlike the other panes it paints
//! through the modern `paint()` path — the designer's render walk special-cases
//! `PLAYBAR_IDX` to `paint_self` (geometry AND text) instead of the legacy
//! flat views, and skips it in `append_frame_text` so the text isn't doubled.

use cce_ui::colors;
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::PaintCtx;
use cce_ui::widget::*;

#[derive(Debug, Clone)]
pub struct Playbar {
    pub playing: bool,
    /// Playback direction while `playing`: reverse runs the frame counter
    /// down and wraps start→end. Simnets re-solve from their seed on every
    /// backward frame (steps are not invertible), exactly like scrubbing.
    pub reversed: bool,
    pub current_frame: f32,
    pub start_frame: f32,
    pub end_frame: f32,
    pub fps: f32,
    /// Whether playback wraps at the range's end (on, the default and the
    /// only behaviour until 2026-09-28) or stops there. Persisted in
    /// state.kdl (`playbar_repeat`) and flipped by `toggle_playbar_repeat`;
    /// this field is the one copy, read by the dialog's switch and
    /// `save_settings`.
    pub repeat: bool,
    /// Whether the Previous Frame / Next Frame buttons flank the play
    /// button — the playbar menu's Step Buttons switch
    /// (`toggle_playbar_step_buttons`), persisted in state.kdl
    /// (`playbar_step_buttons`). On by default. Off, the play button and
    /// the track stand where they always did.
    pub step_buttons: bool,
    /// The window lip the transport stands clear of, on the left, the right
    /// and the bottom: the pane is a SHELF of the window's edge (see the
    /// PLAYBAR_IDX render arm), its rect running out to the window's sides
    /// and bottom, and the lip is drawn over that band. Zero where the
    /// playbar is laid out anywhere else.
    pub frame: f32,
    /// The frames the simulations hold, as runs: drawn as a strip along
    /// the foot of the track, in the accent where they are cached and in
    /// amber where they are stale. Set by the app from the sim cache
    /// (`State::sync_playbar_cache`); empty with no simulation.
    pub cache: Vec<CacheRun>,
    dragging: bool,
}

/// What the simulations hold of a run of frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// Solved, of the chain as it is: playing or scrubbing here costs at
    /// most a checkpoint interval's steps.
    Cached,
    /// Solved, but of the chain as it was before an edit.
    Stale,
}

/// Frames `from..=to` and what is held of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheRun {
    pub from: i32,
    pub to: i32,
    pub state: CacheState,
}

/// The stale strip's colour: the amber a bypassed node wears.
const STALE_COLOR: [f32; 4] = [1.0, 0.74, 0.18, 0.85];

const PAD: f32 = 8.0;
/// Space between two transport buttons.
const BTN_GAP: f32 = 4.0;
/// Width reserved right of the track for the frame readout.
const READOUT_W: f32 = 110.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Btn {
    Prev,
    Play,
    Next,
}

impl Playbar {
    pub fn new() -> Adapted<Playbar> {
        Adapted::new(Self {
            playing: false,
            reversed: false,
            current_frame: 1.0,
            start_frame: 1.0,
            end_frame: 240.0,
            fps: 24.0,
            repeat: true,
            step_buttons: true,
            frame: 0.0,
            cache: Vec::new(),
            dragging: false,
        })
    }

    /// Start playing in a direction. With Repeat off a timeline stopped at
    /// its far end has nowhere to go, so the press restarts it from the
    /// near one, the way a transport's play does after a stop-at-end; with
    /// Repeat on the next tick wraps anyway and the frame is left alone.
    pub fn begin(&mut self, reversed: bool) {
        self.playing = true;
        self.reversed = reversed;
        if !self.repeat {
            if !reversed && self.current_frame >= self.end_frame {
                self.current_frame = self.start_frame;
            } else if reversed && self.current_frame <= self.start_frame {
                self.current_frame = self.end_frame;
            }
        }
    }

    /// Step one whole frame, off the ROUNDED current frame: during playback
    /// the playhead sits between frames, and stepping from the fractional
    /// value would land off the frame grid. The chords and the buttons
    /// share it; neither pauses a playing timeline.
    pub fn step(&mut self, by: f32) {
        self.current_frame = (self.current_frame.round() + by).clamp(self.start_frame, self.end_frame);
    }

    /// The part of the pane the transport is laid out in: the pane less the
    /// window lip on its left, right and bottom ([`Playbar::frame`]).
    fn content(&self, rect: Rect) -> Rect {
        let f = self.frame;
        Rect { x: rect.x + f, y: rect.y, width: (rect.width - 2.0 * f).max(0.0), height: (rect.height - f).max(0.0) }
    }

    /// The transport's square buttons, left to right: Previous Frame, Play,
    /// Next Frame with the step buttons on, Play alone with them off. Glyph
    /// buttons, so square at the toolkit's button height (clamped to the
    /// pane), centred in the strip.
    fn button_rects(&self, rect: Rect) -> Vec<(Btn, Rect)> {
        let s = cce_ui::layout::button_height().min(rect.height).max(12.0);
        let y = rect.y + (rect.height - s) * 0.5;
        let order: &[Btn] = if self.step_buttons { &[Btn::Prev, Btn::Play, Btn::Next] } else { &[Btn::Play] };
        order
            .iter()
            .enumerate()
            .map(|(i, b)| (*b, Rect { x: rect.x + PAD + i as f32 * (s + BTN_GAP), y, width: s, height: s }))
            .collect()
    }

    fn button_at(&self, rect: Rect, x: f32, y: f32) -> Option<Btn> {
        self.button_rects(rect)
            .into_iter()
            .find(|(_, b)| x >= b.x && x <= b.x + b.width && y >= b.y && y <= b.y + b.height)
            .map(|(btn, _)| btn)
    }

    /// Where a transport button is in the pane, for tests: Previous Frame
    /// is -1, Play 0 and Next Frame 1. `None` for a button not shown.
    pub fn transport_button_rect(&self, rect: Rect, which: i32) -> Option<Rect> {
        let want = match which {
            -1 => Btn::Prev,
            0 => Btn::Play,
            _ => Btn::Next,
        };
        self.button_rects(self.content(rect)).into_iter().find(|(b, _)| *b == want).map(|(_, r)| r)
    }

    fn track_rect(&self, rect: Rect) -> Rect {
        let b = self.button_rects(rect).last().expect("the play button").1;
        let x = b.x + b.width + PAD;
        let h = (rect.height - 2.0 * PAD).max(8.0).min(16.0);
        Rect {
            x,
            y: rect.y + (rect.height - h) * 0.5,
            width: (rect.x + rect.width - READOUT_W - PAD - x).max(20.0),
            height: h,
        }
    }

    /// The playhead's normalized position over the frame range.
    fn t(&self) -> f32 {
        let range = self.end_frame - self.start_frame;
        if range <= 0.0 {
            0.0
        } else {
            ((self.current_frame - self.start_frame) / range).clamp(0.0, 1.0)
        }
    }

    fn scrub_to(&mut self, px: f32, track: Rect) {
        let t = ((px - track.x) / track.width.max(1.0)).clamp(0.0, 1.0);
        self.current_frame = (self.start_frame + t * (self.end_frame - self.start_frame)).round();
    }
}

impl Layout for Playbar {}

impl Paint for Playbar {
    /// Subtree painter: `paint` authors the complete pane — geometry and text —
    /// so its Text prims pass through `paint_self` verbatim instead of the
    /// single-font own-labels re-derivation.
    fn paints_own_subtree(&self) -> bool {
        true
    }

    /// The pane IS its own plate, exactly the ParametersBg contract: the
    /// parameter plate's fill — tint, opacity, and blur-behind marker
    /// (`param_plate_fill`) — so it tracks the configured plate tint
    /// (`style.surface.param.color`) with the other panes, where the old
    /// hand-rolled PARAM_BG copy froze this pane at the built-in default.
    fn color(&self) -> [f32; 4] {
        colors::param_plate_fill()
    }

    fn solid_border(&self) -> Option<([f32; 4], f32)> {
        colors::plate_border_color().map(|bc| (bc, colors::plate_border_thickness()))
    }

    fn corner_style(&self, _rect: Rect) -> Option<(f32, (bool, bool, bool, bool))> {
        let r = cce_ui::layout::plate_corner_radius();
        if r > 0.0 {
            Some((r, (true, true, true, true)))
        } else {
            None
        }
    }

    fn paint(&self, rect: Rect, ctx: &mut PaintCtx) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let rect = self.content(rect);
        let relief = cce_ui::layout::control_relief();

        // The transport buttons: raised plates under the DE relief styling
        // (the Button transparent-fill degradation — edges only, the pane
        // plate is the face), flat outlines otherwise.
        let icon = [0.85, 0.86, 0.90, 0.95];
        for (btn, b) in self.button_rects(rect) {
            let br = cce_ui::layout::button_corner_radius().min(b.width * 0.5);
            if relief {
                let depth = cce_ui::layout::bevel_width().min(b.height * 0.2);
                ctx.boss(b, (br, br, br, br), depth);
            } else {
                ctx.border(b, (br, br, br, br), [0.0; 4], [0.35, 0.35, 0.42, 0.9], 1.0);
            }
            match btn {
                Btn::Play => self.paint_play_icon(b, icon, ctx),
                Btn::Prev | Btn::Next => paint_step_icon(b, btn == Btn::Next, icon, ctx),
            }
        }
        // Timeline track: the toolkit's band (the one slider style) — a band the
        // width of the track with its swell at the playhead, in its own shaded
        // well, drawn by the Slider's painter.
        let track = self.track_rect(rect);
        let px = track.x + self.t() * track.width;
        let band_color = if self.dragging { colors::slider_thumb_drag() } else { colors::slider_thumb() };
        cce_ui::widget::input::slider::paint_band_shape(
            ctx,
            track.x,
            track.width,
            track.y + track.height * 0.5,
            band_color,
            &|x| cce_ui::widget::input::slider::band_profile(track.x, track.width, track.height, x, &[px], None),
        );

        // What the simulations hold: a strip along the foot of the track,
        // under the ticks. A frame k is the span k ± 0.5, so a run of one
        // frame is as wide as a frame and the runs meet edge to edge.
        if !self.cache.is_empty() {
            let range = (self.end_frame - self.start_frame).max(1.0);
            let at = |f: f32| track.x + ((f - self.start_frame) / range * track.width).clamp(0.0, track.width);
            let accent = colors::highlight_primary_color();
            let h = 3.0f32.min(track.height * 0.25);
            for run in &self.cache {
                let x0 = at(run.from as f32 - 0.5);
                let x1 = at(run.to as f32 + 0.5);
                if x1 - x0 <= 0.0 {
                    continue;
                }
                let color = match run.state {
                    CacheState::Cached => [accent[0], accent[1], accent[2], 0.85],
                    CacheState::Stale => STALE_COLOR,
                };
                ctx.quad(Rect { x: x0, y: track.y + track.height - h, width: x1 - x0, height: h }, color);
            }
        }

        // Tick marks: frame steps on the 1-2-5 ladder, grown until minors sit
        // >=6px apart. Every 5th step is a major — taller, brighter, and
        // labeled with its frame number when the pane has room below the
        // track and majors aren't crowded.
        let range = (self.end_frame - self.start_frame).max(1.0);
        let ppf = track.width / range;
        let mut step = 1.0f32;
        let cycle = [2.0f32, 2.5, 2.0];
        let mut ci = 0;
        while step * ppf < 6.0 {
            step *= cycle[ci % 3];
            ci += 1;
        }
        let major = step * 5.0;
        let minor_col = [0.82, 0.84, 0.90, 0.30];
        let major_col = [0.87, 0.89, 0.94, 0.55];
        let below = rect.y + rect.height - (track.y + track.height);
        let label_room = below >= 14.0 && major * ppf >= 34.0;
        let mut f = (self.start_frame / step).ceil() * step;
        while f <= self.end_frame + 0.001 {
            let x = track.x + ((f - self.start_frame) / range) * track.width;
            let is_major = (f / major - (f / major).round()).abs() < 1e-3;
            if is_major {
                ctx.quad(Rect { x: x - 0.5, y: track.y, width: 1.0, height: track.height + 3.0 }, major_col);
                if label_room && x + 24.0 <= track.x + track.width {
                    ctx.text(format!("{}", f.round() as i64), x + 3.0, track.y + track.height + 2.0, 9.0, [0x8a, 0x8a, 0x96]);
                }
            } else {
                ctx.quad(
                    Rect { x: x - 0.5, y: track.y + track.height * 0.45, width: 1.0, height: track.height * 0.55 },
                    minor_col,
                );
            }
            f += step;
        }

        // No playhead line: the band's swell already stands at the current
        // frame (until 2026-10-06 an accent line was drawn over it too).

        // Frame readout.
        let text = format!("{:>4} / {}", self.current_frame.round() as i64, self.end_frame.round() as i64);
        let font_size = 12.0;
        let tx = rect.x + rect.width - READOUT_W;
        let ty = cce_ui::layout::align_text_y(rect.y, rect.height, font_size, 0.0);
        ctx.text(text, tx, ty, font_size, [0xcc, 0xcc, 0xd4]);
    }
}

impl Playbar {
    /// The play button's glyph: `pause` while playing, else `play`.
    fn paint_play_icon(&self, b: Rect, icon: [f32; 4], ctx: &mut PaintCtx) {
        ctx.icon(if self.playing { "pause" } else { "play" }, glyph_rect(b), icon);
    }
}

/// A step button's glyph: cce-icons' `step-back` / `step-forward`, the
/// transport's frame-step pair.
fn paint_step_icon(b: Rect, forward: bool, icon: [f32; 4], ctx: &mut PaintCtx) {
    ctx.icon(if forward { "step-forward" } else { "step-back" }, glyph_rect(b), icon);
}

/// Where a transport glyph stands in its button: a square half the
/// button's shorter side, centred — the footprint the hand-drawn triangle
/// and bars had. A missing icon set draws nothing; there is no character
/// to fall back to that is not itself a symbol.
fn glyph_rect(b: Rect) -> Rect {
    let side = (b.width.min(b.height) * 0.5).round();
    Rect { x: (b.x + (b.width - side) * 0.5).round(), y: (b.y + (b.height - side) * 0.5).round(), width: side, height: side }
}

impl Input for Playbar {
    fn is_dragging(&self) -> bool {
        self.dragging
    }

    fn on_event(&mut self, event: &Event, ectx: &mut EventCtx) -> bool {
        let rect = self.content(ectx.rect);
        match event {
            Event::MouseButton { button: MouseButton::Left, state, x, y, .. } => match state {
                ElementState::Pressed => {
                    match self.button_at(rect, *x, *y) {
                        // The play button is the FORWARD transport: playing
                        // (either direction) pauses; paused starts forward.
                        // Reverse is the Down-arrow chord's domain.
                        Some(Btn::Play) => {
                            if self.playing {
                                self.playing = false;
                            } else {
                                self.begin(false);
                            }
                            return true;
                        }
                        Some(Btn::Prev) => {
                            self.step(-1.0);
                            return true;
                        }
                        Some(Btn::Next) => {
                            self.step(1.0);
                            return true;
                        }
                        None => {}
                    }
                    let t = self.track_rect(rect);
                    // A generous vertical band around the slim track.
                    if *x >= t.x && *x <= t.x + t.width && *y >= rect.y && *y <= rect.y + rect.height {
                        self.dragging = true;
                        self.scrub_to(*x, t);
                        return true;
                    }
                    false
                }
                ElementState::Released => std::mem::take(&mut self.dragging),
            },
            Event::PointerMove { x, .. } => {
                if self.dragging {
                    let t = self.track_rect(rect);
                    self.scrub_to(*x, t);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn tick(&mut self, dt: f32, _rect: Rect) -> bool {
        if !self.playing {
            return false;
        }
        let dir = if self.reversed { -1.0 } else { 1.0 };
        let shown = self.current_frame.round();
        let mut next = self.current_frame + dir * dt * self.fps;
        // Every frame is played: a tick moves the shown frame by one at
        // most, landing ON the next frame when the time since the last tick
        // would carry it further. So the rate is the fps while the frames
        // keep up, and slows to a frame a tick when they do not — a
        // simulation too slow for its rate plays every step slower rather
        // than showing some of them. Until 2026-10-06 the playhead kept to
        // the clock and the frames between two ticks were never drawn.
        if (next.round() - shown).abs() > 1.0 {
            next = shown + dir;
        }
        if !self.repeat && (next >= self.end_frame && !self.reversed || next <= self.start_frame && self.reversed) {
            // Repeat off: land ON the last frame and stop there, so the
            // final state of a simulation is what stays on screen.
            self.current_frame = if self.reversed { self.start_frame } else { self.end_frame };
            self.playing = false;
        } else if self.repeat {
            // The loop is every frame of the range, the end and the start
            // each shown for a frame (frame k is shown over k ± 0.5), so the
            // step past the end is to the start and the one before the start
            // is to the end, in either direction.
            let lo = self.start_frame - 0.5;
            let frames = (self.end_frame - self.start_frame + 1.0).max(1.0);
            self.current_frame = lo + (next - lo).rem_euclid(frames);
        } else {
            self.current_frame = next.clamp(self.start_frame, self.end_frame);
        }
        true
    }
}
