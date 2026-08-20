
use cce_ui::colors;
use cce_ui::widget::WidgetHost;

use crate::app::{
    State, FsNode, WIDGET_COUNT,
    CONTENT_IDX, VIEWPORT_IDX, PARAM_IDX,
    BREADCRUMB_IDX, HEADER_IDX, RIGHT_MENUBAR_IDX,
    SPREADSHEET_MENUBAR_IDX, SPREADSHEET_IDX,
    LEFT_MENUBAR_IDX, PARAM_MENUBAR_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX,
};
use crate::geometry::network_sphere_vertices_with_errors;
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::{DisplayList, PaintCtx, Prim};
use cce_ui::scene::painter::{append_widget_plate, append_widget_plate_tinted, append_widget_text};

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
            let base_key = if i == VIEWPORT_IDX || i == NETWORK_PANEL_IDX {
                -5
            } else if i == CONTENT_IDX || i == PARAM_IDX {
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

        // Root Backplate DISSOLVED (Phase 6as): register the widgets (registry consumers:
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
        self.append_popovers(&mut pc);

        // The node right-click context menu floats above everything (drawn last).
        // Its labels carry bounds equal to the menu rect so the engine's text-
        // occlusion clamp (which registers the menu rect) exempts them.
        if cce_ui::widget::context_menu::is_visible() {
            for (qx, qy, qw, qh, qc) in cce_ui::widget::context_menu::extra_quads() {
                pc.quad(rect(qx, qy, qw, qh), qc);
            }
            let mx = cce_ui::widget::context_menu::x();
            let my = cce_ui::widget::context_menu::y();
            let bounds = Some([
                mx,
                my,
                mx + cce_ui::widget::context_menu::w(),
                my + cce_ui::widget::context_menu::h(),
            ]);
            for l in cce_ui::widget::context_menu::text_labels() {
                pc.text_with(l.text, l.x, l.y, l.font_size, l.color, None, bounds);
            }
        }

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
            append_widget_plate(w, pc);
            w.paint_self(&self.ui_context, pc);
        } else if idx == VIEWPORT_IDX {
            // The viewport wears the plate bevel's rim only. It can't be a real
            // `Bevel` plate: the SDF plate draw owns fill AND roll together, so a
            // transparent fill kills the roll and the params fill would frost the
            // whole 3D scene behind its blur marker. A `Boss` step is the rim
            // alone — a fill-less overlay of translucent light/shadow over the
            // scene. Same gate as the plated panes (plate border + control_relief)
            // so the DE style flips together; the radius follows the window
            // curvature (the viewport's corners sit on the window's).
            if cce_ui::layout::control_relief() && cce_ui::colors::plate_border_color().is_some() {
                let (px, py, pw, ph) = self.positions[VIEWPORT_IDX];
                if pw > 0.0 && ph > 0.0 {
                    let r = colors::backplate_corner_radius() * cce_ui::layout::corner_span_factor();
                    let vp_rect = rect(px, py, pw, ph);
                    let radii = (r, r, r, r);
                    let depth = cce_ui::colors::plate_bevel_width();
                    // Focus marks through the rim's specular tint, exactly the
                    // plated panes' treatment (plate_focus_tint).
                    if let Some(tint) = self.plate_focus_tint(idx) {
                        pc.boss_edges_tinted(vp_rect, radii, depth, (true, true, true, true), tint);
                    } else {
                        pc.boss_edges(vp_rect, radii, depth, (true, true, true, true));
                    }
                }
            }
            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                pc.quad(rect(qx, qy, qw, qh), qc);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                pc.circle(cx, cy, cr, cc);
            }
        } else if idx == CONTENT_IDX {
            if !self.circular_network_pane {
                append_widget_plate_tinted(w, pc, self.plate_focus_tint(idx));
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
                // stays a flat quad.
                let cell_r = self.graph().cell_corner_radius();
                let mut bodies: Vec<(f32, f32, f32, f32, bool)> = Vec::new();
                let mut overlays: Vec<(f32, f32, f32, f32, [f32; 4])> = Vec::new();
                let mut seen_node = false;
                for (qx, qy, qw, qh, qc, cell) in self.graph().geometry_quads_tagged(clip) {
                    if self.graph().is_node_rect(qx, qy, qw, qh) {
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

            if show_cursor {
                let px = self.positions[CONTENT_IDX].0;
                let py = self.positions[CONTENT_IDX].1;
                let cx = px + self.grid_cursor_col as f32 * (self.grid_size_x + self.gap_col_w) + self.pan_x;
                let cy = py + self.grid_cursor_row as f32 * (self.grid_size_y + self.gap_row_h) + self.pan_y;
                let cw = self.grid_size_x;
                let ch = self.grid_size_y;
                let thickness = 2.0;
                let mut color = colors::highlight_primary_color();
                color[3] = 0.9;
                if self.graph().is_node_rect(cx, cy, cw, ch) {
                    let r = cce_ui::layout::graph_node_corner_radius();
                    pc.border(rect(cx, cy, cw, ch), (r, r, r, r), [0.0; 4], color, thickness);
                } else {
                    // The empty-cell cursor follows the cells' superellipse arcs.
                    let r = self.graph().cell_corner_radius();
                    pc.clip(clip, |pc| {
                        pc.border(rect(cx, cy, cw, ch), (r, r, r, r), [0.0; 4], color, thickness);
                    });
                }
            }
        } else {
            // The params scrollbar sinks behind the pane plate when idle and rises above the
            // pane content when active (dragged / recently scrolled / hovered). It straddles
            // the plate here rather than riding the pane's `extra_quads`, so its depth can
            // change without touching the rest of the pane chrome.
            let param_scrollbar = if idx == PARAM_IDX {
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
            } else {
                None
            };

            // Idle: draw the scrollbar first so the translucent pane plate settles over it.
            // Track and thumb are pills — half-width radius on the DE corner
            // family (squircle when corner_shape > 2), like the nodes.
            if let Some((quads, false)) = &param_scrollbar {
                for &(qx, qy, qw, qh, qc) in quads {
                    pc.rounded_rect(rect(qx, qy, qw, qh), qw.min(qh) * 0.5, (true, true, true, true), qc);
                }
            }

            append_widget_plate_tinted(w, pc, self.plate_focus_tint(idx));

            // The params pane serves its chrome through the legacy plain-quad view,
            // which carries flat quads only — the controls' rounded-rect backgrounds
            // (textbox/dropdown/button/toggle/color) come from the rounded view and the
            // section outlines' corner fillets from the arc view, drawn under the flat
            // chrome and clipped to the pane's scroll viewport.
            if idx == PARAM_IDX {
                let (px, py, pw, ph) = self.positions[PARAM_IDX];
                let view = rect(px, py + 4.0, pw, (ph - 8.0).max(0.0));
                let param_bg = self
                    .slots
                    .param
                    .as_any()
                    .downcast_ref::<cce_ui::widget::ParametersBg>()
                    .expect("PARAM_IDX must be a ParametersBg");
                pc.clip(view, |pc| {
                    for (qx, qy, qw, qh, qr, qc, corners) in param_bg.rounded_quads(&self.ui_context) {
                        pc.rounded_rect(rect(qx, qy, qw, qh), qr, corners, qc);
                    }
                    for (acx, acy, ar, at, a0, a1, ac) in param_bg.arcs() {
                        pc.arc(acx, acy, ar, at, a0, a1, ac);
                    }
                    // The controls' relief steps (control_relief styling), after the
                    // flat quads so the walls shade the fills they cross.
                    for (rx, ry, rw, rh, radii, rd, raised, edges) in param_bg.reliefs() {
                        if raised {
                            pc.boss_edges(rect(rx, ry, rw, rh), radii, rd, edges);
                        } else {
                            pc.recess_edges(rect(rx, ry, rw, rh), radii, rd, edges);
                        }
                    }
                    // The section carves' concave throat fillets — the inside
                    // corners the box reliefs can't round.
                    for (fcx, fcy, fr, fd, fs) in param_bg.section_fillets() {
                        pc.concave_fillet(fcx, fcy, fr, fd, fs, false);
                    }
                    // The slider thumbs (Prim::Sphere — no flat view carries
                    // them), after the reliefs so the knob rides the carve.
                    for (scx, scy, sr, sc) in param_bg.spheres() {
                        pc.sphere(scx, scy, sr, sc);
                    }
                    // Scene-path rows (ramp curves) — geometry no flat view
                    // carries; drawn last so they sit over the section wells.
                    param_bg.paint_scene_rows(pc);
                });
            }

            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                pc.quad(rect(qx, qy, qw, qh), qc);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                pc.circle(cx, cy, cr, cc);
            }

            // Active: draw the scrollbar last so it rides above the pane content and plate.
            if let Some((quads, true)) = &param_scrollbar {
                for &(qx, qy, qw, qh, qc) in quads {
                    pc.rounded_rect(rect(qx, qy, qw, qh), qw.min(qh) * 0.5, (true, true, true, true), qc);
                }
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
                self.positions[NETWORK_PANEL_IDX]
            }
            RIGHT_MENUBAR_IDX => {
                if !self.show_viewport || relief {
                    return;
                }
                self.positions[VIEWPORT_IDX]
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
        // The viewport's corners sit on the window's, so its highlight follows
        // the window clip's curvature-matched span; the interior panes keep the
        // nominal plate radius their own plates are drawn with.
        let r = if self.focused_pane == RIGHT_MENUBAR_IDX {
            colors::backplate_corner_radius() * cce_ui::layout::corner_span_factor()
        } else {
            cce_ui::layout::plate_corner_radius()
        };
        pc.border(rect(x, y, w, h), (r, r, r, r), [0.0; 4], color, thickness);
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
            // The playbar's text is already in the geometry pass (subtree
            // painter via paint_self — see paint_widget's PLAYBAR_IDX branch).
            if is_menubar || i == PLAYBAR_IDX {
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

    pub(crate) fn rebuild_scene_geometry(&mut self) {
        let mut ocl_error = None;
        let geom = network_sphere_vertices_with_errors(&self.fs_root, &mut ocl_error);

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

        if let Some(e) = ocl_error {
            self.update_status_text(&format!("OpenCL Error: {}", e));
        } else if has_visible_opencl(&self.fs_root) {
            self.update_status_text("OpenCL kernel executed successfully.");
        } else {
            self.update_status_text("Geometry updated successfully.");
        }

        let verts = geom.to_vertex3d_vec();
        self.vertex_count_spheres = verts.len() as u32;
        // Cache for the path tracer, so RT mode never re-runs the node
        // graph / OpenCL kernels; the version bump invalidates its scene.
        // The raster mesh uploads from this same cache on the next
        // `stage_renderer` flush.
        self.rt_sphere_verts = verts;
        self.spheres_dirty = true;
        self.rt_geometry_version += 1;
        self.viewport_dirty = true;
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
