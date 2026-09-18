
use cce_ui::colors;
use cce_ui::widget::WidgetHost;

use crate::app::{State, FsNode};
use crate::slots::{
    WIDGET_COUNT,
    CONTENT_IDX, VIEWPORT_IDX, PARAM_IDX,
    BREADCRUMB_IDX, HEADER_IDX, RIGHT_MENUBAR_IDX,
    SPREADSHEET_MENUBAR_IDX, SPREADSHEET_IDX,
    LEFT_MENUBAR_IDX, PARAM_MENUBAR_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX,
};
use crate::geometry::network_sphere_vertices_with_errors;
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::{DisplayList, PaintCtx, Prim};
use cce_ui::scene::painter::{append_widget_plate, append_widget_plate_radii, append_widget_text};

const TAU: f32 = 2.0 * std::f32::consts::PI;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, width: w, height: h }
}

/// Intersect two logical `[l, t, r, b]` text bounds.
fn merge_bounds(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> Option<[f32; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

impl State {
    /// Per-corner plate radii for a pane rect: a corner that sits ON a window
    /// corner is this pane's share of the window silhouette — the compositor
    /// clips the window at the span-widened root plate arc, so the pane wears
    /// that arc there (what a full-window root plate does in one piece).
    /// Interior corners keep the widget-scale nominal plate radius, matching
    /// the squircles of the sibling panes around them.
    fn pane_plate_radii(&self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        // Delegates to the toolkit since RFC Phase 7b moved this math into
        // PlateSpec: window-corner arcs follow the SHARED silhouette curve
        // (never the app-merged plate override), interior corners the nominal
        // plate radius. This function was the reference implementation.
        use cce_ui::scene::paint::PlateSpec;
        let flags = PlateSpec::window_corner_flags(
            cce_ui::scene::layout::Rect { x, y, width: w, height: h },
            self.width,
            self.height,
        );
        PlateSpec::radii_for(flags)
    }

    /// The rounded-rect clip (plate rect + corner radius) a widget's content must stay
    /// inside, so children of plates cut off at the plate's rounded corners: the network
    /// plate for the network pane's parts (graph content, breadcrumb), the pane's own
    /// plate for the params and spreadsheet panes. `None` with square corners, and for
    /// the circular network pane (which clips by circle instead).
    fn plate_rounded_clip(&self, idx: usize) -> Option<(Rect, f32)> {
        let r = cce_ui::layout::plate_corner_radius();
        if r <= 0.0 {
            return None;
        }
        let (x, y, w, h) = match idx {
            CONTENT_IDX | BREADCRUMB_IDX if !self.circular_network_pane => {
                self.positions[NETWORK_PANEL_IDX]
            }
            crate::slots::CONTENT2_IDX | crate::slots::BREADCRUMB2_IDX => {
                self.positions[crate::slots::NETWORK_PANEL2_IDX]
            }
            PARAM_IDX => self.positions[PARAM_IDX],
            SPREADSHEET_IDX => self.positions[SPREADSHEET_IDX],
            _ => return None,
        };
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some((rect(x, y, w, h), r))
    }

    /// The frame's entire 2D content — geometry AND text — as one display list (the
    /// engine's single paint path; `Application::display_list_text` opts the designer's
    /// text into the engine's shaping/glyph pass, so the app-side FontSystem and buffer
    /// cache are gone). Draw order is the hand-maintained slot order the vertex path
    /// used; the circular network pane rides `PaintItem::clip_circle`.
    pub(crate) fn collect_display_list(&mut self) -> DisplayList {
        // Refresh popover registration: the engine's text-occlusion clamp
        // reads `ui_context.active_popovers` to keep underlying text from
        // bleeding through an open popup's plate. The legacy render_widget
        // helper registered these as a side effect; the designer's own paint
        // walk must do it explicitly or open dropdowns get no occlusion.
        self.ui_context.clear_popovers();
        for i in 0..WIDGET_COUNT {
            if self.slots.get_dyn(i).visible() && self.slots.get_dyn(i).popover_rect().is_some() {
                self.ui_context.register_popover(self.slots.get_dyn_mut(i));
            }
        }

        let show_cursor = self.drag_widget.is_none() && self.app_drag.is_none();

        // The graph content clip (node quads, cursor) and the circular pane clip.
        let clip = if self.circular_network_pane {
            rect(
                self.circular_network_layout.x - self.circular_network_layout.r,
                self.circular_network_layout.y - self.circular_network_layout.r,
                2.0 * self.circular_network_layout.r,
                2.0 * self.circular_network_layout.r,
            )
        } else {
            let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
            rect(cx, cy, cw, ch)
        };
        let clip_circle = if self.circular_network_pane {
            Some([
                self.circular_network_layout.x,
                self.circular_network_layout.y,
                self.circular_network_layout.r,
            ])
        } else {
            None
        };

        let mut draw_order: Vec<usize> = (0..WIDGET_COUNT).collect();
        draw_order.sort_by_key(|&i| {
            let base_key = if i == VIEWPORT_IDX
                || i == NETWORK_PANEL_IDX
                || i == crate::slots::NETWORK_PANEL2_IDX
            {
                -5
            } else if i == CONTENT_IDX || i == crate::slots::CONTENT2_IDX || i == PARAM_IDX {
                -4
            } else if i == HEADER_IDX
                || i == LEFT_MENUBAR_IDX
                || i == RIGHT_MENUBAR_IDX
                || i == PARAM_MENUBAR_IDX
                || i == SPREADSHEET_MENUBAR_IDX
            {
                -3
            } else {
                self.slots.get_dyn(i).z_index()
            };
            (self.has_any_open_menu(i), base_key)
        });

        // root plate container DISSOLVED (Phase 6as): register the widgets (registry consumers:
        // coverage/parent walks) and paint each top-level widget directly in sorted order.
        self.ui_context.clear_hierarchy();
        let widget_ptrs: Vec<*mut (dyn WidgetHost + 'static)> = (0..WIDGET_COUNT)
            .map(|i| self.slots.get_dyn(i) as *const (dyn WidgetHost + 'static) as *mut (dyn WidgetHost + 'static))
            .collect();
        // Register ALL slots, visible or not (id-rooted router): the wheel loop and the
        // hidden-widget broadcasts dispatch by id over the whole roster, and visibility
        // gates behavior inside the widget — an unregistered hidden root would drop the
        // event before that gate.
        for i in 0..WIDGET_COUNT {
            let w = self.slots.get_dyn(i);
            self.ui_context.register_widget(w.base().id(), w as *const (dyn WidgetHost + 'static) as *mut (dyn WidgetHost + 'static));
        }

        let mut pc = PaintCtx::new();
        let mut visited = vec![false; WIDGET_COUNT];
        for &i in &draw_order {
            if !self.slots.get_dyn(i).visible() {
                continue;
            }
            unsafe {
                self.paint_element(&*widget_ptrs[i], &mut pc, show_cursor, &mut visited, clip, clip_circle);
            }
        }

        self.append_context_border(&mut pc);
        self.append_frame_text(&mut pc);
        self.append_meta_point_numbers(&mut pc);
        self.append_scale_readout(&mut pc);
        self.append_curve_tool_overlay(&mut pc);
        self.append_popovers(&mut pc);
        self.append_dock_drag_overlay(&mut pc);
        self.append_plate_corners(&mut pc);

        // The context menu (node/viewport right-click AND the plate corner
        // menus — one shared state) floats above everything, drawn last as
        // the toolkit's lit plate: rounded, translucent, frosted — the
        // material every other floating surface wears. NOT the legacy
        // extra_quads loop, which is the square opaque pre-frost look.
        // Its labels carry bounds equal to the menu rect so the engine's text-
        // occlusion clamp (which registers the menu rect) exempts them.
        // One call for plate and labels: a TextLabel carries no family, so the
        // hand-rolled `paint` + `text_labels()` pair here passed None and drew
        // the menu in the default sans instead of the DE's menu font.
        cce_ui::widget::context_menu::paint_with_labels(&mut pc);

        pc.finish()
    }

    fn paint_widget(
        &self,
        idx: usize,
        pc: &mut PaintCtx,
        show_cursor: bool,
        visited: &mut [bool],
        clip: Rect,
        clip_circle: Option<[f32; 3]>,
    ) {
        if idx >= visited.len() || visited[idx] {
            return;
        }
        visited[idx] = true;

        let w = self.slots.get_dyn(idx);
        if !w.visible() {
            return;
        }

        // A collapsed pane is its title stub and nothing else: the plate, the
        // name, and (from the later corner pass) the control that restores it.
        // Returning here is what suppresses the body — the params rows, the
        // spreadsheet grid, the transport controls — rather than relying on
        // each pane's own clip to hide content taller than the stub.
        if let Some(stub_label) = self.pane_stub_label(idx) {
            let (sx, sy, sw, sh) = w.rect();
            append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(sx, sy, sw, sh));
            let font_size = 12.0;
            let ty = cce_ui::layout::align_text_y(sy, sh, font_size, 0.0);
            // Bounds stop at the corner control so a long name cannot run under it.
            let text_right = sx + sw - 2.0 * crate::plate_corner::CORNER_INSET;
            pc.text_with(
                stub_label,
                sx + 12.0,
                ty,
                font_size,
                [0xcc, 0xcc, 0xd4],
                None,
                Some([sx, sy, text_right, sy + sh]),
            );
            return;
        }

        let is_network_part = idx == CONTENT_IDX || idx == LEFT_MENUBAR_IDX || idx == BREADCRUMB_IDX || idx == NETWORK_PANEL_IDX;
        let active_circle = if is_network_part { clip_circle } else { None };
        if let Some(c) = active_circle {
            pc.push_clip_circle(c);
        }

        // Children of plates cut off at the plate's rounded corners (the param pane's
        // popovers escape this: they render later via `append_popovers`).
        let rounded_clip = self.plate_rounded_clip(idx);
        if let Some((rc, rr)) = rounded_clip {
            pc.push_clip_rounded(rc, rr);
        }

        if idx == NETWORK_PANEL_IDX && self.circular_network_pane {
            let cx = self.circular_network_layout.x;
            let cy = self.circular_network_layout.y;
            let r = self.circular_network_layout.r;
            // Circles carry no blur-behind marker (only Plate/Bevel prims do) — a
            // negative alpha from the params fill would render garbage, so clamp it.
            let mut fill = w.color();
            fill[3] = fill[3].abs();
            pc.circle(cx, cy, r, fill);
            pc.arc(cx, cy, r, 3.0, 0.0, TAU, [0.35, 0.65, 0.95, 0.80 * self.network_opacity]);
        } else if idx == PLAYBAR_IDX {
            // Modern-paint pane: the plate from the legacy views like the other
            // panes, then paint_self emits the transport controls — geometry AND
            // text (a subtree painter; append_frame_text skips this slot so the
            // text isn't doubled).
            let (wx, wy, ww2, wh2) = w.rect();
            append_widget_plate_radii(w, pc, None, self.pane_plate_radii(wx, wy, ww2, wh2));
            w.paint_self(&self.ui_context, pc);
        } else if idx == BREADCRUMB_IDX || idx == crate::slots::BREADCRUMB2_IDX {
            // Modern-paint control: Breadcrumb's whole look lives in its
            // Paint::paint() (the cce-ui restyle — per-segment plates on the
            // dropdown's relief, slanted seams) and it serves NO legacy views,
            // so the fall-through branch rendered bare labels on the network
            // plate. Unlike the playbar it is not a subtree painter: paint_self
            // drops the widget's Text prims, and the labels keep coming from
            // append_frame_text like every other slot — no doubling.
            w.paint_self(&self.ui_context, pc);
        } else if idx == SPREADSHEET_IDX {
            // Modern-paint pane: the plate from the designer (span-widened
            // radii + focus tint), then Spreadsheet::paint authors the grid —
            // header band, zebra rows, separators, dividers, scrollbar. A
            // subtree painter since cce-ui@f1cd939: its text passes through
            // paint_self verbatim, carrying the per-column clamp bounds the
            // own-labels bridge would drop; append_frame_text skips the slot.
            let (wx, wy, ww2, wh2) = w.rect();
            append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(wx, wy, ww2, wh2));
            w.paint_self(&self.ui_context, pc);
        } else if idx == VIEWPORT_IDX {
            // The scene viewer's lip is the window's own root plate edge: the
            // 3D canvas is full-bleed (CANVAS_IDX covers the window; the other
            // panes float over it), so the lip spans the WHOLE window with the
            // window radius on all four corners (the SHARED silhouette value,
            // concentric with the compositor clip). Under control_relief it is
            // the fill-less ROLL OVERLAY (negative-depth Plate): exactly the
            // roll other windows' root plates wear — same width, profile,
            // crest and specular, full band inside the silhouette — screened
            // over the 3D scene, since a filled Plate would cover it (and a
            // frosting fill would blur it). It replaced the Boss rim, whose
            // boundary-straddling wall lost its outer half to the compositor
            // clip: the visible band ran half a roll wide and started at
            // mid-slope. Focus adds the fill-less tinted Bevel — the wrapped
            // accent glint on the same silhouette, the network cursor's prim —
            // matching the focused plates' treatment (shading unchanged, glint
            // in accent). Without control_relief it degrades to the flat
            // plate-border stroke, exactly a bordered plate's outline→relief
            // degradation, and append_context_border owns the focus ring.
            if let Some(bc) = cce_ui::colors::plate_border_color() {
                let (px, py, pw, ph) = (0.0, 0.0, self.width, self.height);
                if pw > 0.0 && ph > 0.0 {
                    let vp_rect = rect(px, py, pw, ph);
                    let radii = self.pane_plate_radii(px, py, pw, ph);
                    if cce_ui::layout::control_relief() {
                        // Window-edge roll width, NOT plate_bevel_width: the lip
                        // matches the root plates of other windows
                        // (style.surface.relief width), not the designer's
                        // interior pane plates.
                        let depth = cce_ui::layout::bevel_width();
                        pc.plate_spec(&cce_ui::scene::paint::PlateSpec {
                            rect: vp_rect,
                            color: [0.0; 4],
                            blur: false,
                            window_corners: (true, true, true, true),
                            depth: -depth,
                        });
                        if let Some(tint) = self.plate_focus_tint(idx) {
                            pc.bevel_tinted(vp_rect, radii, [0.0; 4], depth, tint);
                        }
                    } else {
                        pc.border(vp_rect, radii, [0.0; 4], bc, cce_ui::colors::plate_border_thickness());
                    }
                }
            }
            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                pc.quad(rect(qx, qy, qw, qh), qc);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                pc.circle(cx, cy, cr, cc);
            }
        } else if idx == CONTENT_IDX || idx == crate::slots::CONTENT2_IDX {
            let second = idx == crate::slots::CONTENT2_IDX;
            // The passed-in `clip` is PANE 1's content rect (computed once,
            // before the walk) — zero whenever pane 1 waits as a tab. The
            // second editor clips to its OWN rect or its whole graph
            // vanishes with pane 1's.
            let clip = if second {
                let (cx2, cy2, cw2, ch2) = self.positions[crate::slots::CONTENT2_IDX];
                rect(cx2, cy2, cw2, ch2)
            } else {
                clip
            };
            if second || !self.circular_network_pane {
                let (wx, wy, ww2, wh2) = w.rect();
                append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(wx, wy, ww2, wh2));
            }

            pc.clip(clip, |pc| {
                // Node bodies wear the parameter plate's fill exactly — same
                // tint, opacity, and blur-behind marker (param_plate_fill) —
                // drawn as beveled mini-plates on the DE corner family. They
                // draw as ONE consecutive run so the renderer's blur snapshot
                // is shared across all of them (one full-frame copy, not one
                // per node). Wires/grid/axes stay flat BEHIND the nodes; the
                // geometry toggles stay flat ON TOP. A selected/dragged node
                // keeps the identical fill but glints via a highlight specular
                // tint on its bevel, so selection still reads.
                let node_r = cce_ui::layout::graph_node_corner_radius();
                let radii = (node_r, node_r, node_r, node_r);
                // Half the plate's roll: a node is far smaller than the pane,
                // so the plate's full bevel width would eat most of the body —
                // a tighter lip keeps the flat face reading.
                let node_bevel = cce_ui::colors::plate_bevel_width() * 0.5;
                let node_fill = cce_ui::colors::param_plate_fill();
                let sel = cce_ui::colors::node_selected_color();
                let drag = cce_ui::colors::node_drag_color();
                let hl = cce_ui::colors::highlight_primary_color();
                let hl_tint = [hl[0], hl[1], hl[2]];
                let same_rgb = |a: [f32; 4], b: [f32; 4]| a[0] == b[0] && a[1] == b[1] && a[2] == b[2];

                // Grid cells arrive tagged with their surviving corners and draw
                // as superellipse tiles, like the desktop grid; everything else
                // stays a flat quad. `g` is THIS pane's graph — the second
                // editor paints its own widget's geometry through the same body.
                let g: &dyn cce_ui::widget::GraphController =
                    if second { &*self.slots.content2 } else { self.graph() };
                let cell_r = g.cell_corner_radius();
                let mut bodies: Vec<(f32, f32, f32, f32, bool)> = Vec::new();
                let mut overlays: Vec<(f32, f32, f32, f32, [f32; 4])> = Vec::new();
                let mut seen_node = false;
                for (qx, qy, qw, qh, qc, cell) in g.geometry_quads_tagged(clip) {
                    if g.is_node_rect(qx, qy, qw, qh) {
                        seen_node = true;
                        bodies.push((qx, qy, qw, qh, same_rgb(qc, sel) || same_rgb(qc, drag)));
                    } else if seen_node {
                        overlays.push((qx, qy, qw, qh, qc));
                    } else if let Some(corners) = cell {
                        pc.rounded_rect(rect(qx, qy, qw, qh), cell_r, corners, qc);
                    } else {
                        pc.quad(rect(qx, qy, qw, qh), qc);
                    }
                }
                // Drop-target glow, from the ANIMATED state (tick_frame owns
                // it): position glides between cells, alpha fades in/out.
                // One Prim::Glow — per-vertex-alpha rings the GPU
                // interpolates, a genuinely smooth vignette (the stacked-rect
                // version banded visibly). Drawn under the node bodies: with
                // grid snap the glow reads as a soft aura around the dragged
                // body.
                if let Some(gl) = self.drop_glow {
                    if !second {
                        pc.glow(
                            rect(gl.x, gl.y, gl.w, gl.h),
                            cell_r,
                            30.0,
                            [1.0, 0.72, 0.80, 0.18 * gl.alpha],
                        );
                    }
                }
                for (qx, qy, qw, qh, highlighted) in bodies {
                    if highlighted {
                        pc.bevel_tinted(rect(qx, qy, qw, qh), radii, node_fill, node_bevel, hl_tint);
                    } else {
                        pc.bevel(rect(qx, qy, qw, qh), radii, node_fill, node_bevel);
                    }
                }
                for (qx, qy, qw, qh, qc) in overlays {
                    pc.quad(rect(qx, qy, qw, qh), qc);
                }
            });

            for (cx, cy, cr, cc) in w.extra_circles() {
                if cx >= clip.x && cx <= clip.x + clip.width && cy >= clip.y && cy <= clip.y + clip.height {
                    pc.circle(cx, cy, cr, cc);
                }
            }

            if show_cursor && !second {
                let px = self.positions[CONTENT_IDX].0;
                let py = self.positions[CONTENT_IDX].1;
                let cx = px + self.grid_cursor_col as f32 * (self.grid_size_x + self.gap_col_w) + self.pan_x;
                let cy = py + self.grid_cursor_row as f32 * (self.grid_size_y + self.gap_row_h) + self.pan_y;
                let cw = self.grid_size_x;
                let ch = self.grid_size_y;
                // The cursor is the focus language: a FILL-LESS tinted plate
                // (transparent bevel + accent tint), which the shader renders
                // as the wrapped glint alone — the plate roll's own specular
                // line, so it traces the SAME superellipse silhouette, radius
                // family, and inset as the nodes and panes. Depth = the
                // plates' bevel width for a matching band.
                let color = colors::highlight_primary_color();
                let tint = [color[0], color[1], color[2]];
                let depth = cce_ui::colors::plate_bevel_width();
                if self.graph().is_node_rect(cx, cy, cw, ch) {
                    let r = cce_ui::layout::graph_node_corner_radius();
                    pc.bevel_tinted(rect(cx, cy, cw, ch), (r, r, r, r), [0.0; 4], depth, tint);
                } else {
                    let r = self.graph().cell_corner_radius();
                    pc.clip(clip, |pc| {
                        pc.bevel_tinted(rect(cx, cy, cw, ch), (r, r, r, r), [0.0; 4], depth, tint);
                    });
                }
            }
        } else if idx == PARAM_IDX {
            // Modern-paint pane: ParametersBg::paint_ui (via paint_self)
            // authors the complete row chrome — wells, arcs, reliefs, throat
            // fillets, thumb spheres, scene rows, flat quads — plus the
            // fonted labels from the own-labels bridge, so append_frame_text
            // skips this slot. The pane keeps three designer-owned pieces:
            // the plate (span-widened radii + focus tint), the viewport clip
            // (a clip inside the widget would not survive paint_self's
            // replay), and the scrollbar straddle — idle it sinks behind the
            // translucent plate, active it rides above the content.
            let param_scrollbar = {
                let pb = self
                    .slots
                    .param
                    .as_any()
                    .downcast_ref::<cce_ui::widget::ParametersBg>()
                    .expect("PARAM_IDX must be a ParametersBg");
                if pb.scrollbar_visible() {
                    Some((pb.scrollbar_quads(), pb.scrollbar_active()))
                } else {
                    None
                }
            };
            // Track and thumb are pills — half-width radius on the DE corner
            // family (squircle when corner_shape > 2), like the nodes.
            if let Some((quads, false)) = &param_scrollbar {
                for &(qx, qy, qw, qh, qc) in quads {
                    pc.rounded_rect(rect(qx, qy, qw, qh), qw.min(qh) * 0.5, (true, true, true, true), qc);
                }
            }

            let (wx, wy, ww2, wh2) = w.rect();
            append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(wx, wy, ww2, wh2));

            let (px, py, pw, ph) = self.positions[PARAM_IDX];
            let view = rect(px, py + 4.0, pw, (ph - 8.0).max(0.0));
            pc.clip(view, |pc| {
                w.paint_self(&self.ui_context, pc);
            });

            if let Some((quads, true)) = &param_scrollbar {
                for &(qx, qy, qw, qh, qc) in quads {
                    pc.rounded_rect(rect(qx, qy, qw, qh), qw.min(qh) * 0.5, (true, true, true, true), qc);
                }
            }
        } else {
            let (wx, wy, ww2, wh2) = w.rect();
            append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(wx, wy, ww2, wh2));

            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                pc.quad(rect(qx, qy, qw, qh), qc);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                pc.circle(cx, cy, cr, cc);
            }
        }

        // Child elements, then the widget's popover on top of them.
        for child_ptr in self.ui_context.tree.children_ptrs(w.base().id()) {
            if let Some(child_idx) = self.find_widget_index(child_ptr as *const ()) {
                self.paint_widget(child_idx, pc, show_cursor, visited, clip, clip_circle);
            } else {
                unsafe {
                    self.paint_element(&*child_ptr, pc, show_cursor, visited, clip, clip_circle);
                }
            }
        }

        if rounded_clip.is_some() {
            pc.pop_clip_rounded();
        }
        if active_circle.is_some() {
            pc.pop_clip_circle();
        }
    }

    fn paint_element(
        &self,
        element: &dyn WidgetHost,
        pc: &mut PaintCtx,
        show_cursor: bool,
        visited: &mut [bool],
        clip: Rect,
        clip_circle: Option<[f32; 3]>,
    ) {
        if !element.visible() {
            return;
        }

        if let Some(idx) = self.find_widget_index(element as *const dyn WidgetHost as *const ()) {
            self.paint_widget(idx, pc, show_cursor, visited, clip, clip_circle);
            return;
        }

        append_widget_plate(element, pc);
        for (qx, qy, qw, qh, qc) in element.extra_quads() {
            pc.quad(rect(qx, qy, qw, qh), qc);
        }
        for (cx, cy, cr, cc) in element.extra_circles() {
            pc.circle(cx, cy, cr, cc);
        }

        for child_ptr in self.ui_context.tree.children_ptrs(element.base().id()) {
            unsafe {
                self.paint_element(&*child_ptr, pc, show_cursor, visited, clip, clip_circle);
            }
        }
    }

    /// Highlight border around the focused context's pane. The per-pane
    /// menubars are hidden in the floating layout, so this border is the
    /// only visual indicator of `focused_pane`.
    /// The focused pane's plate carries the highlight as its bevel's specular
    /// tint under `control_relief` — the ring in `append_context_border` is the
    /// flat-style treatment (the viewport's rim-only Boss overlay tints the
    /// same way). Only the circular pane (arc ring) keeps the ring in both
    /// styles.
    fn plate_focus_tint(&self, idx: usize) -> Option<[f32; 3]> {
        if !cce_ui::layout::control_relief() {
            return None;
        }
        let focused = match idx {
            NETWORK_PANEL_IDX | CONTENT_IDX => {
                self.focused_pane == LEFT_MENUBAR_IDX && !self.circular_network_pane
            }
            // The second network editor shares the network focus domain —
            // whichever of the two is FRONTED wears the ring when it holds.
            crate::slots::NETWORK_PANEL2_IDX | crate::slots::CONTENT2_IDX => {
                self.focused_pane == LEFT_MENUBAR_IDX
            }
            VIEWPORT_IDX => self.focused_pane == RIGHT_MENUBAR_IDX,
            PARAM_IDX => self.focused_pane == PARAM_MENUBAR_IDX,
            SPREADSHEET_IDX => self.focused_pane == SPREADSHEET_MENUBAR_IDX,
            _ => false,
        };
        focused.then(|| {
            let c = colors::highlight_primary_color();
            [c[0], c[1], c[2]]
        })
    }

    fn append_context_border(&self, pc: &mut PaintCtx) {
        if self.is_detached_network {
            return;
        }
        // Plated panes under control_relief mark focus through their bevel's
        // specular tint (plate_focus_tint) — no ring on top of it.
        let relief = cce_ui::layout::control_relief();

        let thickness = 2.0;
        let mut color = colors::highlight_primary_color();
        color[3] = 0.9;

        let (x, y, w, h) = match self.focused_pane {
            LEFT_MENUBAR_IDX => {
                if !self.show_network || self.detached_circular_network {
                    return;
                }
                if self.circular_network_pane {
                    color[3] *= self.network_opacity;
                    pc.arc(
                        self.circular_network_layout.x,
                        self.circular_network_layout.y,
                        self.circular_network_layout.r,
                        3.0,
                        0.0,
                        TAU,
                        color,
                    );
                    return;
                }
                if relief {
                    return;
                }
                color[3] *= self.network_opacity;
                // Whichever network editor is FRONTED owns the ring — pane
                // 1's rect is zero while it waits as a tab.
                let p2 = self.positions[crate::slots::NETWORK_PANEL2_IDX];
                if p2.2 > 0.0 && self.positions[NETWORK_PANEL_IDX].2 <= 0.0 {
                    p2
                } else {
                    self.positions[NETWORK_PANEL_IDX]
                }
            }
            RIGHT_MENUBAR_IDX => {
                if !self.show_viewport || relief {
                    return;
                }
                // The scene viewer's rim is the whole window root plate (see the
                // VIEWPORT_IDX paint branch); its focus highlight follows it.
                (0.0, 0.0, self.width, self.height)
            }
            PARAM_MENUBAR_IDX => {
                if !self.show_parameters || relief {
                    return;
                }
                self.positions[PARAM_IDX]
            }
            SPREADSHEET_MENUBAR_IDX => {
                if !self.show_spreadsheet || relief {
                    return;
                }
                self.positions[SPREADSHEET_IDX]
            }
            _ => return,
        };

        if w <= 0.0 || h <= 0.0 {
            return;
        }
        // The highlight follows the pane plate's arcs exactly: window-corner
        // corners at the window clip's curvature-matched span, interior
        // corners at the nominal plate radius (pane_plate_radii).
        pc.border(rect(x, y, w, h), self.pane_plate_radii(x, y, w, h), [0.0; 4], color, thickness);
    }

    /// The frame's text, as `Prim::Text` items shaped and drawn by the engine
    /// (`display_list_text`): each non-menubar widget's walk-derived labels — the
    /// graph's clamped to the network pane (and distance-filtered against the circular
    /// pane), network text fading with `network_opacity`. Popovers follow in
    /// `append_popovers`.
    fn append_frame_text(&self, pc: &mut PaintCtx) {
        let circular = self.circular_network_pane;
        let ncx = self.circular_network_layout.x;
        let ncy = self.circular_network_layout.y;
        let ncr = self.circular_network_layout.r;

        for i in 0..WIDGET_COUNT {
            let w = self.slots.get_dyn(i);
            if !w.visible() {
                continue;
            }
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            // Panes on the paint_self path carry their text in the geometry
            // pass already (see paint_widget: subtree text for the playbar
            // and spreadsheet, the own-labels bridge for the params pane) —
            // drawing them here again would double it.
            if is_menubar || i == PLAYBAR_IDX || i == PARAM_IDX || i == SPREADSHEET_IDX {
                continue;
            }
            let is_node = i == CONTENT_IDX;
            let is_network_part = i == CONTENT_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;

            // Widget-level clip bounds (logical px), matching the old TextBounds.
            let widget_bounds = if is_node {
                if circular {
                    Some([ncx - ncr, ncy - ncr, ncx + ncr, ncy + ncr])
                } else {
                    let (gx, gy, gw, gh) = self.positions[CONTENT_IDX];
                    Some([gx, gy, gx + gw, gy + gh])
                }
            } else {
                None
            };

            // Labels are plate children too — clip them at the plate's rounded corners.
            let rounded_clip = self.plate_rounded_clip(i);
            if let Some((rc, rr)) = rounded_clip {
                pc.push_clip_rounded(rc, rr);
            }
            let mut scratch = PaintCtx::new();
            append_widget_text(&self.ui_context, w, &mut scratch);
            for item in scratch.finish().items {
                if let Prim::Text { text, x, y, font_size, color, font, bounds: label_bounds, .. } = item.prim {
                    if circular && is_network_part {
                        let dx = x - ncx;
                        let dy = y - ncy;
                        if dx * dx + dy * dy > ncr * ncr {
                            continue;
                        }
                    }
                    // Node text belongs to the node domain: it fades with
                    // node_opacity, not the pane's network_opacity.
                    let alpha = if is_node {
                        self.node_opacity.clamp(0.0, 1.0)
                    } else if is_network_part {
                        self.network_opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    pc.text_faded(text, x, y, font_size, color, alpha, font, merge_bounds(widget_bounds, label_bounds));
                }
            }
            if rounded_clip.is_some() {
                pc.pop_clip_rounded();
            }
        }

    }

    /// Open popovers (the params pane's expanded dropdowns), background then
    /// text per widget. Appended after `append_frame_text` so the popover
    /// occludes the widget labels underneath it — the display list is drawn
    /// strictly in order, so a popover background emitted in the geometry
    /// pass would sit under every label.
    /// The plates' corner menu triggers, drawn above pane content but below an
    /// open context menu: a solid dot in the plate's border color — one color,
    /// like the graph's port dots and geometry toggles — growing slightly on
    /// hover (and while its menu is open) instead of changing tint.
    /// The dock-drop highlight while a plate is being dragged by its dot: the
    /// region the release would snap it into, tinted and outlined.
    fn append_dock_drag_overlay(&self, pc: &mut PaintCtx) {
        let (Some(crate::app::AppDrag::DockDrag { .. }), Some(target)) = (self.app_drag, self.dock_drag_target) else {
            return;
        };
        let (x, y, w, h) = self.dock_rect(target);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let hl = colors::highlight_primary_color();
        let r = cce_ui::layout::plate_corner_radius();
        pc.rounded_rect(rect(x, y, w, h), r, (true, true, true, true), [hl[0], hl[1], hl[2], 0.12]);
        pc.border(rect(x, y, w, h), (r, r, r, r), [0.0; 4], [hl[0], hl[1], hl[2], 0.8], 2.0);
    }

    fn append_plate_corners(&self, pc: &mut PaintCtx) {
        let hovered = self.plate_corner_at(self.cursor_x, self.cursor_y);
        for idx in crate::plate_corner::PLATE_SLOTS {
            let Some(c) = self.plate_corner_center(idx) else { continue };
            cce_ui::widget::plate_dock::draw_corner_dot(
                pc,
                c,
                hovered == Some(idx) || self.plate_menu_slot == Some(idx),
            );
        }
    }

    fn append_popovers(&self, pc: &mut PaintCtx) {
        for i in 0..WIDGET_COUNT {
            let w = self.slots.get_dyn(i);
            if !w.visible() {
                continue;
            }
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if is_menubar {
                continue;
            }
            if self.focused_widget == Some(i) || i == PARAM_IDX {
                let mut popover_pc = cce_ui::layout::PopoverCollector::new();
                w.render_popover(&mut popover_pc);
                for (color, px, py, pw, ph) in popover_pc.rects {
                    pc.quad(rect(px, py, pw, ph), color);
                }
                for (t, size, x, y, tc, font_opt, label_bounds) in popover_pc.texts {
                    let color = [
                        (tc[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (tc[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (tc[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                    ];
                    pc.text_with(t, x, y, size, color, font_opt, label_bounds);
                }
            }
        }
    }

    /// The meta "Point Numbers" overlay: each collected (position, index)
    /// label projects through the raster scene's cached mvp into 2D text,
    /// clipped to the viewport pane. The mvp cache refreshes whenever the
    /// camera or pane changes (`stage_frame`), so the labels track orbits;
    /// a frame staged before the first scene staging simply draws none.
    fn append_meta_point_numbers(&self, pc: &mut PaintCtx) {
        if !self.show_viewport || self.meta_number_labels.is_empty() {
            return;
        }
        let Some(mvp) = self.last_scene_mvp else { return };
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            for (pos, idx) in &self.meta_number_labels {
                let clip_pos = mvp * glam::Vec4::new(pos[0], pos[1], pos[2], 1.0);
                if clip_pos.w <= 0.0 {
                    continue;
                }
                let ndc = clip_pos / clip_pos.w;
                if ndc.x.abs() > 1.02 || ndc.y.abs() > 1.02 {
                    continue;
                }
                let sx = vx + (ndc.x * 0.5 + 0.5) * vw;
                let sy = vy + (0.5 - ndc.y * 0.5) * vh;
                pc.text(idx.to_string(), sx + 4.0, sy - 6.0, 10.0, [0xee, 0xee, 0xff]);
            }
        });
    }

    /// The view's scale on the pivot plane, bottom-left of the pane: `1:2.3`
    /// (the world shown at less than true size), `2.3:1` (magnified), or
    /// `1:1`, with what one world unit is and how long it shows. Marked when
    /// the display metric is only assumed — then the millimetres are the
    /// CSS 96 ppi guess, not a measurement.
    fn append_scale_readout(&self, pc: &mut PaintCtx) {
        if !self.show_viewport {
            return;
        }
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let r = self.view_scale_ratio();
        if !r.is_finite() || r <= 0.0 {
            return;
        }
        let ratio = if (r - 1.0).abs() < 0.01 {
            "1:1".to_string()
        } else if r > 1.0 {
            format!("1:{}", cce_ui::units::fmt_num((r * 100.0).round() / 100.0))
        } else {
            format!("{}:1", cce_ui::units::fmt_num((100.0 / r).round() / 100.0))
        };
        let m = cce_ui::units::metric();
        let shown_mm = self.world_unit_mm() / r;
        let mut text = format!("{ratio}  ·  1 {} = {} mm on screen", self.world_unit.suffix(), cce_ui::units::fmt_num((shown_mm * 100.0).round() / 100.0));
        if !m.is_real() {
            text.push_str("  ·  metric assumed");
        }
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            pc.text(text, vx + 8.0, vy + vh - 16.0, 10.0, [0xaa, 0xaa, 0xbb]);
        });
    }

    /// The curve viewer state's handles: each control point projected
    /// through the cached scene mvp (like the point numbers above), drawn as
    /// a ringed dot with its index, the control cage as faint segments
    /// between them. Selected point draws larger and brighter.
    fn append_curve_tool_overlay(&self, pc: &mut PaintCtx) {
        let Some(tool) = &self.curve_tool else { return };
        if !self.show_viewport {
            return;
        }
        let handles = self.curve_tool_handles();
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            for pair in handles.windows(2) {
                let (_, x0, y0, _) = pair[0];
                let (_, x1, y1, _) = pair[1];
                pc.vector(x0, y0, x1, y1, 1.0, [1.0, 1.0, 1.0, 0.25], cce_ui::scene::paint::Cap::Round);
            }
            for (i, sx, sy, _z) in &handles {
                let selected = tool.selected == Some(*i);
                let r = if selected { 6.0 } else { 4.5 };
                // Dark ring behind for contrast against any scene.
                pc.circle(*sx, *sy, r + 1.5, [0.0, 0.0, 0.0, 0.6]);
                let col = if selected {
                    [1.0, 0.92, 0.55, 1.0]
                } else {
                    [1.0, 0.78, 0.20, 1.0]
                };
                pc.circle(*sx, *sy, r, col);
                pc.text((i + 1).to_string(), sx + 8.0, sy - 6.0, 10.0, [0xff, 0xe6, 0xa0]);
            }
        });
    }

    pub(crate) fn rebuild_scene_geometry(&mut self) {
        let mut ocl_error = None;
        // The sim cache lives on State so playing forward steps each simnet once
        // per frame instead of re-solving its whole history every rebuild.
        let (frame, start) = (self.sim_frame(), self.sim_start_frame());
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        let geom = {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            // The viewport shows ITS editor's level — the pinned one when a
            // pin is set, else whichever editor took the last node click —
            // while name resolution stays rooted at fs_root. Navigation in
            // the bound editor re-scopes this.
            network_sphere_vertices_with_errors(&self.fs_root, self.viewport_editor_dir(), &mut ocl_error, &mut sim)
        };
        self.sim_cache = sim_cache;

        fn has_visible_opencl(node: &FsNode) -> bool {
            if node.node_type.eq_ignore_ascii_case("opencl") && node.geometry_visible {
                return true;
            }
            for child in &node.children {
                if has_visible_opencl(child) {
                    return true;
                }
            }
            false
        }

        let displayed_opencl = has_visible_opencl(self.viewport_editor_dir());
        if let Some(e) = ocl_error {
            self.update_status_text(&format!("OpenCL Error: {}", e));
        } else if displayed_opencl {
            self.update_status_text("OpenCL kernel executed successfully.");
        } else {
            self.update_status_text("Geometry updated successfully.");
        }

        let verts = crate::geometry::detail_vertices(&geom);
        self.vertex_count_spheres = verts.len() as u32;
        // Cache for the path tracer, so RT mode never re-runs the node
        // graph / OpenCL kernels; the version bump invalidates its scene.
        // The raster mesh uploads from this same cache on the next
        // `stage_renderer` flush.
        self.rt_sphere_verts = verts;
        self.spheres_dirty = true;
        self.rt_geometry_version += 1;
        self.viewport_dirty = true;

        // Per-node meta overlays ride the same rebuild: markers and point
        // numbers for nodes whose meta child asks for them.
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        let (markers, labels, wires, normals) = {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            // Same scoping as the scene walk above: overlays annotate what is
            // on screen, so they walk the same current level.
            collect_meta_overlays(&self.fs_root, self.viewport_editor_dir(), self.meta_marker_size, self.meta_marker_color, &mut sim)
        };
        self.sim_cache = sim_cache;
        self.meta_marker_verts = markers;
        self.meta_number_labels = labels;
        self.meta_wire_verts = wires;
        self.meta_normal_verts = normals;
        self.meta_points_dirty = true;
    }

    /// The path tracer's scene: the sphere geometry (and the reference cube if
    /// shown) as triangles, with one Lambertian material per distinct vertex
    /// color. Same mesh space as the raster pass, so the raster mvp's inverse
    /// drives the camera.
    pub(crate) fn collect_rt_scene(
        &self,
    ) -> (Vec<cce_ui::vk::RtTriangle>, Vec<cce_ui::vk::RtMaterial>) {
        let mut verts = self.rt_sphere_verts.clone();
        if self.viewport().show_cube {
            verts.extend(crate::geometry::cube_vertices());
        }
        crate::geometry::rt_scene_from_verts(&verts)
    }

    pub(crate) fn update_status_text(&mut self, text: &str) {
        if self.last_status_text != text {
            self.last_status_text = text.to_string();
            self.slots.status.set_text(text);
        }
    }
}

/// The per-node meta (preferences) overlay walk: for every node whose `meta`
/// child asks for Point Markers or Point Numbers — and whose geometry is
/// visible through the same parent chain the scene walk uses — evaluate the
/// node and collect marker geometry and/or (position, vertex index) labels.
/// Positions dedupe the triangle soup's repeats; a label keeps the FIRST
/// index at its position, matching the spreadsheet's vertex numbering.
/// `start` scopes the walk to the displayed network level (the scene walk's
/// contract — pass `root` for both to cover the whole tree); evaluation
/// stays rooted at `root`.
pub(crate) fn collect_meta_overlays(
    root: &FsNode,
    start: &FsNode,
    point_size: f32,
    marker_color: [f32; 3],
    sim: &mut crate::geometry::EvalSim,
) -> (
    Vec<crate::geometry::Vertex3D>,
    Vec<([f32; 3], u32)>,
    Vec<crate::geometry::Vertex3D>,
    Vec<crate::geometry::Vertex3D>,
) {
    let mut markers = Vec::new();
    let mut labels = Vec::new();
    let mut wires = Vec::new();
    let mut normals = Vec::new();
    fn visit(
        root: &FsNode,
        node: &FsNode,
        parent_visible: bool,
        point_size: f32,
        marker_color: [f32; 3],
        markers: &mut Vec<crate::geometry::Vertex3D>,
        labels: &mut Vec<([f32; 3], u32)>,
        wires: &mut Vec<crate::geometry::Vertex3D>,
        normals: &mut Vec<crate::geometry::Vertex3D>,
        sim: &mut crate::geometry::EvalSim,
    ) {
        let is_visible = parent_visible && node.geometry_visible;
        let want_markers = is_visible && crate::app::meta_pref(node, "Point Markers");
        let want_numbers = is_visible && crate::app::meta_pref(node, "Point Numbers");
        let want_wires = is_visible && crate::app::meta_pref(node, "Wireframe");
        let want_normals = is_visible && crate::app::meta_pref(node, "Point Normals");
        if want_markers || want_numbers || want_wires || want_normals {
            let mut visited = Vec::new();
            let mut err = None;
            if let Some(geom) = crate::geometry::generate_single_node_geometry_with_errors(
                root, node, &mut visited, &mut err, sim,
            ) {
                if want_wires {
                    // The geometry's own edge list as LINE_LIST pairs, carrying
                    // its own colors. Two changes from the soup version, both
                    // of them the topology finally being visible: an edge two
                    // faces share is drawn once instead of twice, and a quad
                    // shows as a quad — the fan diagonal was never an edge of
                    // the mesh, only of its triangulation.
                    for e in geom.edges() {
                        for &p in e {
                            let p = p as usize;
                            wires.push(crate::geometry::Vertex3D {
                                position: geom.positions()[p],
                                color: geom.color(p),
                            });
                        }
                    }
                }
                if want_markers {
                    // One marker per point. The soup emitted one per corner and
                    // leaned on points_vertices deduping by position.
                    let src: Vec<crate::geometry::Vertex3D> = geom
                        .positions()
                        .iter()
                        .map(|&position| crate::geometry::Vertex3D { position, color: [0.0; 3] })
                        .collect();
                    markers.extend(crate::geometry::points_vertices(
                        &src,
                        point_size,
                        cce_ui::colors::to_linear_rgb(marker_color),
                    ));
                }
                if want_normals {
                    // Smooth point normals: for each point, the normalized sum
                    // of the face normals of the primitives touching it.
                    // Template meshes wind CCW seen from outside (the raster
                    // culling convention), so the plain cross(B-A, C-A) points
                    // outward. The kernel outputs' Norm attribute is a default
                    // up-vector — useless here.
                    //
                    // The soup had to reconstruct "which triangles touch this
                    // point" by hashing quantized positions, every frame. That
                    // was a weld in all but name, and it is what point_prims
                    // answers directly.
                    use glam::Vec3;
                    let len = point_size * 4.0;
                    let color = cce_ui::colors::to_linear_rgb([0.45, 0.8, 1.0]);
                    for p in 0..geom.num_points() {
                        let mut sum = Vec3::ZERO;
                        for &prim in geom.point_prims(p) {
                            let pts = geom.prim_points(prim as usize);
                            if pts.len() < 3 {
                                continue;
                            }
                            let a = geom.pos(pts[0] as usize);
                            let b = geom.pos(pts[1] as usize);
                            let c = geom.pos(pts[2] as usize);
                            let n = (b - a).cross(c - a);
                            if n.length_squared() > 1e-12 {
                                sum += n;
                            }
                        }
                        let n = sum.normalize_or_zero();
                        if n == Vec3::ZERO {
                            continue;
                        }
                        let pos = geom.positions()[p];
                        let tip = geom.pos(p) + n * len;
                        normals.push(crate::geometry::Vertex3D { position: pos, color });
                        normals.push(crate::geometry::Vertex3D { position: tip.to_array(), color });
                    }
                }
                if want_numbers {
                    // The point's index, which is now also its spreadsheet row.
                    // The soup numbered by first-corner-at-this-position, so
                    // the overlay and the spreadsheet disagreed.
                    for p in 0..geom.num_points() {
                        labels.push((geom.positions()[p], p as u32));
                    }
                }
            }
        }
        for c in &node.children {
            visit(root, c, is_visible, point_size, marker_color, markers, labels, wires, normals, sim);
        }
    }
    for c in &start.children {
        visit(root, c, true, point_size, marker_color, &mut markers, &mut labels, &mut wires, &mut normals, sim);
    }
    // A dense mesh can label tens of thousands of points; the text pass is
    // per-frame, so cap it rather than melt the frame rate.
    const MAX_LABELS: usize = 2000;
    if labels.len() > MAX_LABELS {
        labels.truncate(MAX_LABELS);
    }
    (markers, labels, wires, normals)
}
