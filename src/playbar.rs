//! The playbar pane: a full-width animation-transport strip docked below the
//! other panes (`PLAYBAR_IDX`). App-owned on the narrow traits wrapped in
//! `Adapted<Playbar>`, like `Viewport3D`. Unlike the other panes it paints
//! through the modern `paint()` path — the designer's render walk special-cases
//! `PLAYBAR_IDX` to `paint_self` (geometry AND text) instead of the legacy
//! flat views, and skips it in `append_frame_text` so the text isn't doubled.

use cce_ui::colors;
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::{Cap, PaintCtx};
use cce_ui::widget::*;

#[derive(Debug, Clone)]
pub struct Playbar {
    pub playing: bool,
    pub current_frame: f32,
    pub start_frame: f32,
    pub end_frame: f32,
    pub fps: f32,
    dragging: bool,
}

const PAD: f32 = 8.0;
/// Width of the play/pause button box (square-ish, clamped to pane height).
const BTN_W: f32 = 28.0;
/// Width reserved right of the track for the frame readout.
const READOUT_W: f32 = 110.0;

impl Playbar {
    pub fn new() -> Adapted<Playbar> {
        Adapted::new(Self {
            playing: false,
            current_frame: 1.0,
            start_frame: 1.0,
            end_frame: 240.0,
            fps: 24.0,
            dragging: false,
        })
    }

    fn button_rect(&self, rect: Rect) -> Rect {
        let s = (rect.height - 2.0 * PAD).max(12.0).min(BTN_W);
        Rect { x: rect.x + PAD, y: rect.y + (rect.height - s) * 0.5, width: s, height: s }
    }

    fn track_rect(&self, rect: Rect) -> Rect {
        let b = self.button_rect(rect);
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

    /// The pane IS its own plate, exactly the ParametersBg contract: PARAM_BG
    /// scaled by the global plate opacity, alpha negated as the blur marker.
    fn color(&self) -> [f32; 4] {
        let mut c = colors::PARAM_BG;
        c[3] *= cce_ui::layout::plate_opacity();
        if colors::plate_blur() {
            c[3] = -c[3].abs();
        }
        c
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
        let relief = cce_ui::layout::control_relief();
        let accent = colors::highlight_primary_color();

        // Play/pause button: raised plate under the DE relief styling (the
        // Button transparent-fill degradation — edges only, the pane plate is
        // the face), flat outline otherwise.
        let b = self.button_rect(rect);
        let br = cce_ui::layout::button_corner_radius().min(b.width * 0.5);
        if relief {
            let depth = cce_ui::layout::bevel_width().min(b.height * 0.2);
            ctx.boss(b, (br, br, br, br), depth);
        } else {
            ctx.border(b, (br, br, br, br), [0.0; 4], [0.35, 0.35, 0.42, 0.9], 1.0);
        }
        let icon = [0.85, 0.86, 0.90, 0.95];
        if self.playing {
            // Pause: two bars.
            let bw = b.width * 0.16;
            let bh = b.height * 0.44;
            let by = b.y + (b.height - bh) * 0.5;
            ctx.quad(Rect { x: b.x + b.width * 0.32 - bw * 0.5, y: by, width: bw, height: bh }, icon);
            ctx.quad(Rect { x: b.x + b.width * 0.68 - bw * 0.5, y: by, width: bw, height: bh }, icon);
        } else {
            // Play: triangle outline (no filled-triangle prim; the DE's line
            // aesthetic reads fine here).
            let (cx, cy) = (b.x + b.width * 0.54, b.y + b.height * 0.5);
            let r = b.width * 0.24;
            let (x0, y0) = (cx - r * 0.6, cy - r);
            let (x1, y1) = (cx - r * 0.6, cy + r);
            let (x2, y2) = (cx + r, cy);
            ctx.vector(x0, y0, x1, y1, 1.5, icon, Cap::Round);
            ctx.vector(x1, y1, x2, y2, 1.5, icon, Cap::Round);
            ctx.vector(x2, y2, x0, y0, 1.5, icon, Cap::Round);
        }

        // Timeline track: the Slider recessed-track look — under control_relief
        // the carve alone defines the channel and the fill sits on its floor.
        let track = self.track_rect(rect);
        let tr = cce_ui::layout::slider_corner_radius().min(track.height * 0.5);
        let recess_t = cce_ui::layout::bevel_width().min(track.height * 0.2);
        if !relief {
            ctx.rounded_rect(track, tr, (true, true, true, true), colors::slider_track());
        }
        if let Some(fill) = colors::slider_fill() {
            let (fx, fy, fmax_w, fh) = if relief {
                let inset = recess_t * 0.5;
                (track.x + inset, track.y + inset, track.width - 2.0 * inset, track.height - 2.0 * inset)
            } else {
                (track.x, track.y, track.width, track.height)
            };
            let fill_w = (self.t() * fmax_w).max(0.0);
            if fill_w > 0.5 {
                ctx.rounded_rect(
                    Rect { x: fx, y: fy, width: fill_w, height: fh },
                    tr.min(fh / 2.0),
                    (true, true, true, true),
                    fill,
                );
            }
        }
        if relief {
            ctx.recess(track, (tr, tr, tr, tr), recess_t);
        }

        // Playhead: a full-height line over the track, in the DE accent.
        let px = track.x + self.t() * track.width;
        ctx.quad(Rect { x: px - 1.0, y: track.y - 3.0, width: 2.0, height: track.height + 6.0 }, accent);

        // Frame readout.
        let text = format!("{:>4} / {}", self.current_frame.round() as i64, self.end_frame.round() as i64);
        let font_size = 12.0;
        let tx = rect.x + rect.width - READOUT_W;
        let ty = cce_ui::layout::align_text_y(rect.y, rect.height, font_size, 0.0);
        ctx.text(text, tx, ty, font_size, [0xcc, 0xcc, 0xd4]);
    }
}

impl Input for Playbar {
    fn is_dragging(&self) -> bool {
        self.dragging
    }

    fn on_event(&mut self, event: &Event, ectx: &mut EventCtx) -> bool {
        let rect = ectx.rect;
        match event {
            Event::MouseButton { button: MouseButton::Left, state, x, y, .. } => match state {
                ElementState::Pressed => {
                    let b = self.button_rect(rect);
                    if *x >= b.x && *x <= b.x + b.width && *y >= b.y && *y <= b.y + b.height {
                        self.playing = !self.playing;
                        return true;
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
        self.current_frame += dt * self.fps;
        if self.current_frame > self.end_frame {
            let range = (self.end_frame - self.start_frame).max(1.0);
            self.current_frame = self.start_frame + (self.current_frame - self.start_frame) % range;
        }
        true
    }
}
