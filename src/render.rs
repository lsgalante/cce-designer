
use cce_ui::colors;
use cce_ui::widget::WidgetHost;

use crate::app::State;
use crate::slots::{
    WIDGET_COUNT,
    CONTENT_IDX, VIEWPORT_IDX, PARAM_IDX,
    BREADCRUMB_IDX, HEADER_IDX, RIGHT_MENUBAR_IDX,
    SPREADSHEET_MENUBAR_IDX, SPREADSHEET_IDX,
    LEFT_MENUBAR_IDX, PARAM_MENUBAR_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX,
    DIALOG_IDX,
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

/// What a bypassed node wears in the network: Houdini's bypass flag is
/// this colour, and nothing else in the pane is.
const BYPASS_TINT: [f32; 3] = [1.0, 0.74, 0.18];

/// The point numbers' font size, logical px.
const POINT_NUMBER_PX: f32 = 10.0;


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
        // What is drawn over the scene is placed by the camera as it is
        // NOW, not as the stage pass last saw it.
        self.refresh_scene_view();
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
        // The dialog's open dropdown, AFTER the dialog and after the wipe
        // above (which would drop it from the tree, leaving an id the
        // engine's clamp cannot resolve): the clamp lets an occluder's own
        // labels through only past the occluders registered before it, so
        // the dialog's labels under the list are clamped and the list's are
        // not.
        if self.dialog_visible() && self.slots.dialog.dropdown.open && self.sync_dialog_dropdown() {
            let dd: &mut (dyn cce_ui::widget::WidgetHost + 'static) = &mut *self.slots.dialog.dropdown;
            self.ui_context.register_popover(dd);
        }

        let mut pc = PaintCtx::new();
        let mut visited = vec![false; WIDGET_COUNT];
        for &i in &draw_order {
            if !self.slots.get_dyn(i).visible() {
                continue;
            }
            // The dialog is painted after the overlay passes below, not in the
            // walk. A high z_index is not enough: `append_frame_text` and the
            // viewport overlays run AFTER the whole walk, so the graph's node
            // labels and the scale readout drew straight over a dialog that
            // had already covered them.
            if i == DIALOG_IDX {
                continue;
            }
            unsafe {
                self.paint_element(&*widget_ptrs[i], &mut pc, show_cursor, &mut visited, clip, clip_circle);
            }
        }

        self.append_context_border(&mut pc);
        self.append_frame_text(&mut pc);
        self.append_point_numbers(&mut pc);
        self.append_scale_readout(&mut pc);
        self.append_viewer_state_overlay(&mut pc);
        self.append_popovers(&mut pc);
        // Above every pane AND every overlay text pass, below only the context
        // menu — which can be opened from inside it.
        self.append_dialog(&mut pc, show_cursor, &mut visited, clip);

        // The context menu (every right-click menu, the plate menus
        // included — one shared state) floats above everything, drawn last as
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

        // A collapsed pane is its title stub and nothing else: the plate and
        // the name. A press on it restores it; a right press, its plate menu.
        // Returning here is what suppresses the body — the params rows, the
        // spreadsheet grid, the transport controls — rather than relying on
        // each pane's own clip to hide content taller than the stub.
        if let Some(stub_label) = self.pane_stub_label(idx) {
            let (sx, sy, sw, sh) = w.rect();
            append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(sx, sy, sw, sh));
            let font_size = 12.0;
            let ty = cce_ui::layout::align_text_y(sy, sh, font_size, 0.0);
            // Bounds stop a margin short of the stub's end.
            let text_right = sx + sw - 12.0;
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
        } else if idx == DIALOG_IDX {
            // Modern-paint surface, the playbar's contract: the designer
            // authors the plate (the dialog floats, so the pane radii and the
            // focus tint do not apply — it is never a pane and never the
            // focused one), then Dialog::paint emits the query line and the
            // rows. A subtree painter, so append_frame_text skips the slot and
            // the chord column keeps its own font and bounds.
            //
            // The plate is THE MENU PLATE — `context_menu::paint_menu_plate`,
            // the one function the context menus draw theirs with: the
            // `style.surface.menu` material, radius and roll. Drawn here
            // rather than through `append_widget_plate` because that helper
            // builds a pane's material from its fill, and the dialog is not
            // a pane; it is a menu that happens to have a query line. Not in
            // a popup surface, so the in-app frost pass compresses its
            // backdrop as the config says rather than folding that into
            // opacity as the hosted menus must.
            let (x, y, ww, h) = w.rect();
            let full = rect(x, y, ww, h);
            match (self.slots.dialog.turn_progress(), self.slots.dialog.turning) {
                (Some(e), Some(turn)) => {
                    // Turning (see `dialog::DialogTurn`): the plate on its way
                    // from the one it replaced, and what it shows sliding in
                    // and coming up inside it. Everything is painted aside
                    // and replayed moved; the text is cut at the plate as it
                    // is drawn, which is the occluder the dialog claims
                    // meanwhile, so the clamp still lets it through.
                    let now = self.slots.dialog.drawn_rect(full);
                    cce_ui::widget::context_menu::paint_menu_plate(pc, now, false);
                    let mut scratch = PaintCtx::new();
                    w.paint_self(&self.ui_context, &mut scratch);
                    let dx = turn.dir * (1.0 - e) * cce_ui::widget::context_menu::TURN_SLIDE;
                    // Bounds are offset by the translate; these land on `now`.
                    let bounds = Some([now.x - dx, now.y, now.x + now.width - dx, now.y + now.height]);
                    let radius = cce_ui::layout::menu_corner_radius();
                    pc.clip_rounded(now, radius, |pc| {
                        pc.translate(dx, 0.0, |pc| {
                            for item in scratch.finish().items {
                                if let Some(c) = item.clip {
                                    pc.push_clip(c);
                                }
                                if let Some(Prim::Text { text, x, y, font_size, color, alpha, font, .. }) = pc.replay(item.prim) {
                                    pc.text_faded(text, x, y, font_size, color, alpha * e * e, font, bounds);
                                }
                                if item.clip.is_some() {
                                    pc.pop_clip();
                                }
                            }
                        });
                    });
                }
                _ => {
                    cce_ui::widget::context_menu::paint_menu_plate(pc, full, false);
                    w.paint_self(&self.ui_context, pc);
                }
            }
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
                        // The window-edge roll width (style.surface.relief
                        // width), read directly as the root plates of other
                        // windows read it. The interior pane plates roll over
                        // the same number through `plate_bevel_width` — one
                        // roll width since 2026-09-28; the `plate.bevel_width`
                        // key that once set theirs apart is retired.
                        let depth = cce_ui::layout::bevel_width();
                        pc.plate_spec(&cce_ui::scene::paint::PlateSpec {
                            rect: vp_rect,
                            material: cce_ui::scene::Material::opaque([0.0; 4]),
                            window_corners: (true, true, true, true),
                            depth: -depth,
                        });
                        if let Some(tint) = self.plate_focus_tint(idx) {
                            pc.bevel_tinted(vp_rect, radii, &cce_ui::scene::Material::from_fill([0.0; 4]), depth, tint);
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
            // No plate here: the pane's plate is NETWORK_PANEL_IDX's
            // PassivePlate (the params material, gated on `network_plate`
            // in the generic arm). Until 2026-09-20 this arm ALSO painted
            // the graph widget's own background over it — the cell colour
            // at the network opacity, a flat blue-grey wash that predated
            // the plate and made the network pane the one pane with a hue.

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
                // The pane material, with the nodes' own compression when
                // configured (State::node_compression): the one knob that
                // differs between a node body and the plate it sits on.
                let node_mat = {
                    let mut m = cce_ui::scene::Material::from_fill(node_fill);
                    if let (Some(k), cce_ui::scene::Frost::Frosted { compression, .. }) = (self.node_compression, &mut m.frost) {
                        *compression = k;
                    }
                    m
                };
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
                // Which of this pane's nodes are bypassed, by slot: the
                // widget knows a node's rect and its slot, the tree knows
                // the flag.
                let bypassed: Vec<bool> = {
                    let level = if second { self.dir_at(&self.current_path2.clone()) } else { self.current_dir() };
                    level.children.iter().map(crate::geometry::is_bypassed).collect()
                };
                let mut bodies: Vec<(f32, f32, f32, f32, bool, bool)> = Vec::new();
                let mut overlays: Vec<(f32, f32, f32, f32, [f32; 4])> = Vec::new();
                let mut seen_node = false;
                // The grid lines (flat, gap colour at the network opacity)
                // and the origin axes, under the wires and nodes; the cells
                // are the pane plate itself. Then the wires, which are
                // strokes in the node wire style and not among the quads.
                g.paint_grid(clip, pc);
                g.paint_wires(clip, pc);
                for (qx, qy, qw, qh, qc, cell) in g.geometry_quads_tagged(clip) {
                    if g.is_node_rect(qx, qy, qw, qh) {
                        seen_node = true;
                        // An EXPANDED cursor selects every node standing
                        // inside it, and they wear the selected look. The
                        // widget colours one body — its own `selected_idx` —
                        // so the rest are recognised here, by the cell the
                        // body is centred on: the same `grid_cursor_covers`
                        // the selection itself is derived from, rather than a
                        // second rect test that could disagree with it. Pane
                        // 1 only, the grid cursor being pane 1's concept.
                        let in_region = !second
                            && self.grid_cursor_expanded()
                            && {
                                let (col, row) = self.cell_at(qx + qw * 0.5, qy + qh * 0.5);
                                self.grid_cursor_covers(col, row)
                            };
                        let off = g.node_at(qx + qw * 0.5, qy + qh * 0.5).is_some_and(|slot| bypassed.get(slot).copied().unwrap_or(false));
                        bodies.push((qx, qy, qw, qh, in_region || same_rgb(qc, sel) || same_rgb(qc, drag), off));
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
                // A bypassed node wears amber: its roll tinted, through the
                // bevel's own tint channel, and a bar down its left side,
                // flat and on top with the geometry toggles. The bar is
                // what still says so while the node is selected and its
                // roll is the selection's colour.
                let mut bars: Vec<cce_ui::scene::layout::Rect> = Vec::new();
                for (qx, qy, qw, qh, highlighted, off) in bodies {
                    if highlighted {
                        pc.bevel_tinted(rect(qx, qy, qw, qh), radii, &node_mat, node_bevel, hl_tint);
                    } else if off {
                        pc.bevel_tinted(rect(qx, qy, qw, qh), radii, &node_mat, node_bevel, BYPASS_TINT);
                    } else {
                        pc.bevel(rect(qx, qy, qw, qh), radii, &node_mat, node_bevel);
                    }
                    if off {
                        let (inset, wide) = (qh * 0.22, (qw * 0.05).max(2.0));
                        bars.push(rect(qx + inset, qy + inset, wide, qh - inset * 2.0));
                    }
                }
                for (qx, qy, qw, qh, qc) in overlays {
                    pc.quad(rect(qx, qy, qw, qh), qc);
                }
                for bar in bars {
                    pc.quad(bar, [BYPASS_TINT[0], BYPASS_TINT[1], BYPASS_TINT[2], 0.9]);
                }
            });

            for (cx, cy, cr, cc) in w.extra_circles() {
                if cx >= clip.x && cx <= clip.x + clip.width && cy >= clip.y && cy <= clip.y + clip.height {
                    pc.circle(cx, cy, cr, cc);
                }
            }

            if show_cursor && !second {
                // A node-sized outline centred on the cursor's intersection —
                // exactly where a node placed there would sit — or, after a
                // drag across the grid, the union of the region it expanded
                // over. One cell is the usual case and the same rect as ever.
                let (cx, cy, cw, ch) = self.grid_cursor_rect();
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
                    pc.bevel_tinted(rect(cx, cy, cw, ch), (r, r, r, r), &cce_ui::scene::Material::from_fill([0.0; 4]), depth, tint);
                } else {
                    let r = self.graph().cell_corner_radius();
                    pc.clip(clip, |pc| {
                        pc.bevel_tinted(rect(cx, cy, cw, ch), (r, r, r, r), &cce_ui::scene::Material::from_fill([0.0; 4]), depth, tint);
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

            // Expression rows carry Houdini's tint: a translucent green over
            // the row, so a driven parameter reads as driven before its text
            // is read. Rows are index-parallel to `param_display`, which is
            // what the pane was handed.
            if let Some(child) = self.param_editor_selected().and_then(|slot| self.param_editor_dir().children.get(slot)) {
                let rows = crate::app::param_display(&child.params);
                let is_expr = |key: &str| {
                    child.params.iter().any(|p| {
                        let k = p.shown_name();
                        k == key && p.is_expr()
                    })
                };
                if rows.iter().any(|r| is_expr(&r.0)) {
                    let rects = self.param_row_rects();
                    pc.clip(view, |pc| {
                        for (row, &(rx, ry, rw, rh)) in rows.iter().zip(rects.iter()) {
                            if rh > 0.0 && is_expr(&row.0) {
                                pc.rounded_rect(rect(rx, ry, rw, rh), 6.0, (true, true, true, true), [0.35, 0.8, 0.45, 0.16]);
                            }
                        }
                    });
                }
            }

            if let Some((quads, true)) = &param_scrollbar {
                for &(qx, qy, qw, qh, qc) in quads {
                    pc.rounded_rect(rect(qx, qy, qw, qh), qw.min(qh) * 0.5, (true, true, true, true), qc);
                }
            }
        } else {
            let (wx, wy, ww2, wh2) = w.rect();
            let network_panel =
                idx == NETWORK_PANEL_IDX || idx == crate::slots::NETWORK_PANEL2_IDX;
            if !(network_panel && !self.network_plate) {
                append_widget_plate_radii(w, pc, self.plate_focus_tint(idx), self.pane_plate_radii(wx, wy, ww2, wh2));
            }

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
            if is_menubar
                || i == PLAYBAR_IDX
                || i == PARAM_IDX
                || i == SPREADSHEET_IDX
                || i == DIALOG_IDX
            {
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

    /// The Alt+D dialog, last of the pane content: its plate and command list,
    /// its settings body, and that body's popovers.
    ///
    /// Out of the widget walk entirely, because the walk is not the end of the
    /// frame — `append_frame_text` and the viewport overlays follow it, and
    /// they drew the graph's node labels and the scale readout straight over
    /// a dialog whose z_index had already put it on top of the same panes.
    /// Only the context menu goes above this, and it can be opened from inside
    /// the dialog.
    fn append_dialog(
        &self,
        pc: &mut PaintCtx,
        show_cursor: bool,
        visited: &mut [bool],
        clip: Rect,
    ) {
        if !self.slots.dialog.visible() {
            return;
        }
        self.paint_widget(DIALOG_IDX, pc, show_cursor, visited, clip, None);
    }

    /// Open popovers (the params pane's expanded dropdowns), background then
    /// text per widget, in the widget's own drawing. Appended after `append_frame_text` so the popover
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
            // Into the frame's own PaintCtx, as cce-files and the palette
            // paint theirs: a dropdown's expanded menu is its trigger's
            // plate grown, relief and corners and all. Until 2026-10-01 it
            // went through a `PopoverCollector`, which keeps fills as plain
            // rects — the menu came out square-cornered and flat, a
            // different thing from the control it grew out of.
            if self.focused_widget == Some(i) || i == PARAM_IDX {
                w.render_popover(pc);
            }
        }
    }

    /// The meta "Point Numbers" overlay: each collected (position, index)
    /// label projects through the raster scene's cached mvp into 2D text,
    /// clipped to the viewport pane. The mvp cache refreshes whenever the
    /// camera or pane changes (`stage_frame`), so the labels track orbits;
    /// a frame staged before the first scene staging simply draws none.
    fn append_point_numbers(&self, pc: &mut PaintCtx) {
        let labels = self.point_number_labels();
        if labels.is_empty() {
            return;
        }
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            for (text, x, y, color, alpha) in labels {
                pc.text_faded(text, x, y, POINT_NUMBER_PX, color, alpha, None, None);
            }
        });
    }

    /// The numbers as they are drawn: text, where, colour and strength.
    ///
    /// **A number under a plate is not drawn.** A plate frosts what is
    /// behind it, and the scene's markers and wires show through one
    /// blurred; but the engine lays ALL text out after all geometry, so a
    /// number under a plate would be drawn over it, sharp, where everything
    /// beside it is frosted. It cannot be blurred with the scene from here,
    /// and under the pane tint a blurred 10 px number would not be read
    /// anyway.
    pub(crate) fn point_number_labels(&self) -> Vec<(String, f32, f32, [u8; 3], f32)> {
        let mut out = Vec::new();
        if !self.show_viewport {
            return out;
        }
        // Points, primitives, vertices: each its own colour, and its own
        // nudge off the place it names — a point's number beside its
        // marker, the other two about on the spot, which is theirs alone.
        let lists: [(&[([f32; 3], u32)], &[f32], [u8; 3], (f32, f32)); 3] = [
            (&self.overlay_number_labels, &self.overlay_number_alpha, [0xee, 0xee, 0xff], (4.0, -6.0)),
            (&self.overlay_prim_labels, &self.overlay_prim_alpha, PRIM_LABEL_COLOR, (-4.0, -6.0)),
            (&self.overlay_vertex_labels, &self.overlay_vertex_alpha, VERTEX_LABEL_COLOR, (-3.0, -5.0)),
        ];
        if lists.iter().all(|(labels, ..)| labels.is_empty()) {
            return out;
        }
        let Some(mvp) = self.last_scene_mvp else { return out };
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        if vw <= 0.0 || vh <= 0.0 {
            return out;
        }
        {
            for (labels, alphas, color, (dx, dy)) in lists {
                for (i, (pos, idx)) in labels.iter().enumerate() {
                    // What the fill in front of the place lets through; a
                    // number behind an opaque face is not drawn.
                    let alpha = alphas.get(i).copied().unwrap_or(1.0);
                    if alpha < 0.02 {
                        continue;
                    }
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
                    let text = idx.to_string();
                    // Both ends of the label, at about its middle height.
                    let (lx, ly) = (sx + dx, sy + dy);
                    let wide = text.len() as f32 * POINT_NUMBER_PX * 0.62;
                    let mid = ly + POINT_NUMBER_PX * 0.6;
                    if self.under_a_plate(lx, mid) || self.under_a_plate(lx + wide, mid) {
                        continue;
                    }
                    out.push((text, lx, ly, color, alpha));
                }
            }
        }
        out
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
    fn append_viewer_state_overlay(&self, pc: &mut PaintCtx) {
        let Some(tool) = &self.viewer_tool else { return };
        if !self.show_viewport {
            return;
        }
        let handles = self.viewer_tool_handles();
        let (vx, vy, vw, vh) = self.last_scene_view_rect;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let outline = self.viewer_tool_outline();
        let cage = tool.source.cage();
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            // What is being edited, under its handles: dark then light, so
            // the line reads over a white sheet and over a black one.
            for i in 0..outline.len() {
                let (x0, y0) = outline[i];
                let (x1, y1) = outline[(i + 1) % outline.len()];
                pc.vector(x0, y0, x1, y1, 2.5, [0.0, 0.0, 0.0, 0.45], cce_ui::scene::paint::Cap::Round);
                pc.vector(x0, y0, x1, y1, 1.0, [1.0, 0.78, 0.20, 0.9], cce_ui::scene::paint::Cap::Round);
            }
            for pair in handles.windows(2).filter(|_| cage) {
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
                // On a dark tab of its own, as the HUD is: a handle can
                // stand over a white image, where light text is no text.
                let label = tool.source.handle_label(*i);
                if !label.is_empty() {
                    let width = label.chars().count() as f32 * 10.0 * 0.52 + 6.0;
                    pc.quad(rect(sx + 5.0, sy - 8.0, width, 14.0), [0.0, 0.0, 0.0, 0.55]);
                    pc.text(label, sx + 8.0, sy - 6.0, 10.0, [0xff, 0xe6, 0xa0]);
                }
            }
        });

        // The HUD sits one line ABOVE the scale readout, sharing its left
        // margin. Not at the top: the viewport is full-bleed and the pane
        // plates float over its top edge, so a mode line there lands under the
        // collapsed stubs and their titles read through it. Not at the very
        // bottom either — that row belongs to the scale readout, and two
        // sentences on one line read as one garbled sentence.
        //
        // It exists because a viewer state changes what every click does and
        // snapping silently changes what a drag does. A mode you cannot see is
        // a mode you forget you are in, and the first symptom is a click that
        // does something surprising.
        let hud = tool.hud();
        let size = 11.0;
        let pad = 5.0;
        let y = vy + vh - 16.0 - (size + pad * 2.0) - 4.0;
        let width = (hud.chars().count() as f32 * size * 0.52 + pad * 2.0).min(vw - 16.0);
        pc.clip(rect(vx, vy, vw, vh), |pc| {
            pc.quad(rect(vx + 8.0 - pad, y, width, size + pad * 2.0), [0.0, 0.0, 0.0, 0.55]);
            pc.text(hud, vx + 8.0, y + pad, size, [0xff, 0xe6, 0xa0]);
        });
    }

    /// Compose the 2D page the displayed level holds, if it holds one, and
    /// hand it to the scene as an image.
    ///
    /// The page context's counterpart to the geometry rebuild, and it runs on
    /// the same trigger for the same reason: a parameter changed, so what the
    /// viewport shows is stale. The raster goes to the GPU as an image whose
    /// id is owned here and freed when it is replaced, so a page that is
    /// being scrubbed does not leak a texture per frame. The stage pass draws
    /// it as a quad standing in the scene (`stage_frame`), where until
    /// 2026-09-29 a pane of its own took the viewport's place.
    pub(crate) fn rebuild_page(&mut self) {
        let level = self.viewport_editor_dir();
        let node_id = crate::page::displayed_page_node(level).map(|n| n.id.clone());
        let page = crate::page::displayed_page(&self.fs_root, level);
        // The same picture with new contents keeps its image: a handle
        // being dragged recomposes the page on every motion, and freeing an
        // image waits for the device to go idle.
        let before = self.page_shown.take();
        let had_page = before.is_some();
        let same_size = |page: &crate::page::Page| {
            before.as_ref().is_some_and(|b| b.pixels == (page.width, page.height))
        };
        let old = self.page_image.take();
        let kept = old.filter(|_| page.as_ref().is_some_and(same_size));
        if let (Some(old), None) = (old, kept) {
            cce_ui::vk::free_image(old);
        }
        if let (Some(page), Some(node_id)) = (page, node_id) {
            let (w, h) = (page.width, page.height);
            self.page_image = Some(match kept {
                Some(id) => {
                    cce_ui::vk::update_pixels(id, page.to_rgba8(), w, h, cce_ui::vk::PixelFormat::Rgba);
                    id
                }
                // Mipmapped: a sheet composed at 300 DPI is drawn at a fifth
                // of its size in an ordinary pane, and its hairlines crawl.
                None => cce_ui::vk::upload_rgba_mipmapped(page.to_rgba8(), w, h),
            });
            self.update_status_text(&format!(
                "Image: {} x {} {} at {} DPI ({}x{} px)",
                trim_number(page.in_unit(page.size[0])),
                trim_number(page.in_unit(page.size[1])),
                page.unit.label(),
                page.dpi,
                w,
                h
            ));
            self.page_shown = Some(crate::page::PageShown {
                node_id,
                size: page.size,
                pixels: (w, h),
                origin: page.origin,
            });
        }
        if had_page || self.page_shown.is_some() {
            self.viewport_dirty = true;
            // The path tracer's scene holds the image too.
            self.page_version += 1;
        }
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

        // A script error names its line; if the node it names is the one the
        // params pane shows, the pane flags that line in its code row. Cleared
        // whenever the evaluation says nothing about that node.
        let flagged = ocl_error.as_deref().and_then(|e| self.code_error_line_for_pane(e));
        self.slots.param_bg_mut().set_code_error_line(flagged);
        if let Some(e) = ocl_error {
            self.update_status_text(&format!("Node error: {}", e));
        } else {
            self.update_status_text("Geometry updated successfully.");
        }

        // What the visualizers are applied to, kept so an edit to one
        // re-presents this scene rather than running the graph again.
        self.scene_attributes = crate::visualizer::scene_attributes(&geom);
        self.scene_base = Some(geom.clone());
        self.present_scene(geom);

        // The pull arrows measure the selected node against this new
        // geometry version; a playing simnet reaches here every frame.
        self.sync_selection_readouts();
        self.sync_pull_arrows();

        // Last, not first: the page's status line would otherwise be
        // overwritten by the geometry pass's own, and a level showing a page
        // has nothing to say about geometry.
        self.rebuild_page();
    }

    /// Show the scene last evaluated again, under the visualizers as they
    /// now stand — what an edit to one runs. Nothing before the first
    /// evaluation.
    /// Read the scene's environment again (`crate::environment`), and when
    /// it moved, redraw: a smooth-shaded fill re-bakes its light from the
    /// scene as last evaluated, which evaluates nothing. Run from the tick,
    /// so a sun driven by `$F`, or an edit that rebuilt nothing, is seen.
    pub(crate) fn sync_environment(&mut self) -> bool {
        let env = crate::environment::Environment::of_scene(&self.fs_root, self.sim_frame());
        if env == self.environment {
            return false;
        }
        self.environment = env;
        if self.smooth_shading {
            self.revisualize();
        }
        self.viewport_dirty = true;
        true
    }

    pub(crate) fn revisualize(&mut self) {
        if let Some(base) = self.scene_base.clone() {
            self.present_scene(base);
        }
    }

    /// Everything the viewport draws of an evaluated scene: the visualizers
    /// applied to it, then its fill, its groups, its overlays and its edges.
    fn present_scene(&mut self, mut geom: crate::detail::Detail) {
        crate::visualizer::apply_all(&self.visualizers, &mut geom);

        let verts = crate::geometry::detail_vertices(&geom);
        self.vertex_count_spheres = verts.len() as u32;
        // Smooth shading bakes the light into a raster copy of the fill;
        // `verts` stays unlit for the path tracer, whose materials are
        // these colours. Same triangles in the same order, so the count
        // above serves both.
        // Lit by the environment's sun, which the flat shader is handed too.
        self.environment = crate::environment::Environment::of_scene(&self.fs_root, self.sim_frame());
        self.scene_smooth_verts = if self.smooth_shading {
            crate::geometry::smooth_lit_vertices(&geom, self.environment.sun_direction)
        } else {
            Vec::new()
        };
        // Cache for the path tracer, so RT mode never re-runs the node
        // graph; the version bump invalidates its scene.
        // The raster mesh uploads from this same cache on the next
        // `stage_renderer` flush.
        self.rt_sphere_verts = verts;
        self.spheres_dirty = true;
        self.rt_geometry_version += 1;
        self.viewport_dirty = true;

        // The scene's point groups, with where their members are: what the
        // Group Markers dialog lists, and what the marked ones' markers are
        // built from.
        self.scene_groups = geom
            .points()
            .group_names()
            .iter()
            .map(|g| (g.to_string(), geom.points().group_members(g).iter().map(|&p| geom.positions()[p as usize]).collect()))
            .collect();
        self.rebuild_marked_group_verts();

        // The point overlays ride the same rebuild, off the same `geom`:
        // they annotate what is on screen, and what is on screen is exactly
        // this Detail.
        let (markers, labels, normals) = scene_point_overlays(
            &geom,
            self.show_point_markers,
            self.show_point_numbers,
            self.show_point_normals,
            self.point_marker_size,
            self.point_marker_color,
        );
        self.overlay_marker_verts = markers;
        // Kept for re-sizing the markers without this evaluation — see
        // `State::rebuild_overlay_marker_verts`.
        self.overlay_marker_points = if self.show_point_markers {
            geom.positions()
                .iter()
                .map(|&position| crate::geometry::Vertex3D { position, color: [0.0; 3] })
                .collect()
        } else {
            Vec::new()
        };
        self.overlay_number_labels = labels;
        self.overlay_number_alpha.clear();
        self.overlay_normal_verts = normals;
        let elements = scene_element_overlays(
            &geom,
            ElementOverlays {
                prim_numbers: self.show_prim_numbers,
                prim_normals: self.show_prim_normals,
                vertex_numbers: self.show_vertex_numbers,
                vertex_markers: self.show_vertex_markers,
                vertex_normals: self.show_vertex_normals,
            },
            self.point_marker_size * 4.0,
        );
        self.overlay_prim_labels = elements.prim_labels;
        self.overlay_prim_alpha.clear();
        self.overlay_vertex_labels = elements.vertex_labels;
        self.overlay_vertex_alpha.clear();
        self.overlay_normal_verts.extend(elements.normals);
        self.overlay_vertex_marker_points = elements.vertex_markers;
        if !self.overlay_vertex_marker_points.is_empty() {
            // Both marker lists, by the one builder the size slider uses.
            self.rebuild_overlay_marker_verts();
        }
        // The wire pass's edges, likewise — topological, and only while the
        // wireframe is actually on.
        self.scene_edge_verts =
            if self.wireframe { scene_edge_verts(&geom) } else { Vec::new() };

        // Visualize's vector markers — a node's or a visualizer's — ride the
        // same LINE_LIST channel as the normal whiskers.
        self.overlay_normal_verts.extend(crate::geometry::vis_marker_vertices(
            &geom,
            cce_ui::colors::to_linear_rgb,
        ));
        self.overlay_dirty = true;
        // The numbers' dimming, for the view the scene was last staged
        // from. The 2D frame is painted BEFORE the stage pass, which works
        // the dimming out: left cleared, the first frame after every
        // rebuild drew each number at full strength, those behind the
        // surface too, and a playing simulation rebuilds every frame — the
        // numbers flickered between shown and dimmed.
        if let Some(mvp) = self.last_scene_mvp {
            self.sync_point_number_alpha(mvp, self.last_scene_eye);
        }
    }

    /// The path tracer's scene: the scene geometry as triangles, with one
    /// Lambertian material per distinct vertex color. Same mesh space as the
    /// raster pass, so the raster mvp's inverse drives the camera.
    pub(crate) fn collect_rt_scene(
        &self,
    ) -> (Vec<cce_ui::vk::RtTriangle>, Vec<cce_ui::vk::RtMaterial>) {
        crate::geometry::rt_scene_from_verts(&self.rt_sphere_verts)
    }

    /// The 0-based line a node error points at in the params pane's selected
    /// node, when the error names that node and carries a `(line N` — the
    /// shape Rhai's diagnostics take (`wrangle1: point 4: ... (line 3,
    /// position 5)`). Anything else is None.
    pub(crate) fn code_error_line_for_pane(&self, error: &str) -> Option<usize> {
        let slot = self.param_editor_selected()?;
        let node_name = &self.param_editor_dir().children.get(slot)?.name;
        let rest = error.strip_prefix(node_name.as_str())?.strip_prefix(':')?;
        Self::error_line_number(rest)
    }

    /// A clipboard, selection or history action for the params pane's code
    /// editor, when one is open. False otherwise, so the caller's own
    /// handling runs.
    pub(crate) fn code_editor_action(&mut self, action: cce_ui::widget::ContextAction) -> bool {
        let pane = self.slots.param_bg_mut();
        pane.code_editing() && pane.code_action(action)
    }

    /// `(line N` anywhere in a diagnostic, 1-based in the text, 0-based out.
    pub(crate) fn error_line_number(text: &str) -> Option<usize> {
        let at = text.find("(line ")?;
        let digits: String = text[at + 6..].chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1)
    }

    pub(crate) fn update_status_text(&mut self, text: &str) {
        if self.last_status_text != text {
            self.last_status_text = text.to_string();
            self.slots.status.set_text(text);
        }
    }
}

/// The point overlays on the displayed scene: marker geometry for Show
/// Point Markers, `(position, index)` labels for Show Point Numbers, and
/// normal whiskers for Show Point Normals — each read straight off the
/// merged scene `Detail` the geometry rebuild has already produced.
///
/// It reads that one `Detail` rather than walking the tree because the
/// overlays are a property of the VIEW, not of individual nodes. While they
/// were per-node `meta` preferences this had to be a second walk that
/// re-evaluated every flagged node on its own — the same repeated evaluation
/// that made a five-flag Embryo cost 2.4 s an edit. The scene is evaluated
/// once now, and the overlays cost a pass over its points.
pub(crate) fn scene_point_overlays(
    geom: &crate::detail::Detail,
    markers_on: bool,
    numbers_on: bool,
    normals_on: bool,
    point_size: f32,
    marker_color: [f32; 3],
) -> (
    Vec<crate::geometry::Vertex3D>,
    Vec<([f32; 3], u32)>,
    Vec<crate::geometry::Vertex3D>,
) {
    let mut markers = Vec::new();
    let mut labels = Vec::new();
    let mut normals = Vec::new();
    if markers_on {
        // One marker per point. The soup emitted one per corner and leaned
        // on points_vertices deduping by position.
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
    if normals_on {
        // Smooth point normals: for each point, the normalized sum of the
        // face normals of the primitives touching it. Template meshes wind
        // CCW seen from outside (the raster culling convention), so the
        // plain cross(B-A, C-A) points outward. The kernel outputs' N
        // attribute is a default up-vector — useless here.
        //
        // The soup had to reconstruct "which triangles touch this point" by
        // hashing quantized positions, every frame. That was a weld in all
        // but name, and it is what point_prims answers directly.
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
    if numbers_on {
        // The point's index, which is also its spreadsheet row. The soup
        // numbered by first-corner-at-this-position, so the overlay and the
        // spreadsheet disagreed.
        //
        // A dense mesh can label tens of thousands of points and the text
        // pass is per-frame, so cap it rather than melt the frame rate.
        const MAX_LABELS: usize = 2000;
        for p in 0..geom.num_points().min(MAX_LABELS) {
            labels.push((geom.positions()[p], p as u32));
        }
    }
    (markers, labels, normals)
}

/// The colours the primitive and vertex overlays are told from the points'
/// by: warm for a primitive's number and its normal, green for a vertex's.
pub(crate) const PRIM_LABEL_COLOR: [u8; 3] = [0xff, 0xc8, 0x8c];
pub(crate) const VERTEX_LABEL_COLOR: [u8; 3] = [0xa0, 0xf0, 0xb0];

/// How far from its point toward its primitive's centroid a vertex's
/// number stands: far enough that the vertices sharing a point are apart,
/// near enough to say which corner each is.
pub(crate) const VERTEX_LABEL_INSET: f32 = 0.3;

/// A vertex marker's radius as a fraction of Point Marker Size.
pub(crate) const VERTEX_MARKER_SCALE: f32 = 0.6;

/// Which of the primitive and vertex overlays to collect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ElementOverlays {
    pub prim_numbers: bool,
    pub prim_normals: bool,
    pub vertex_numbers: bool,
    pub vertex_markers: bool,
    pub vertex_normals: bool,
}

/// What `scene_element_overlays` collected: `(position, index)` labels for
/// the two kinds of number, the places the vertex markers stand, and the
/// LINE_LIST whiskers of both kinds of normal.
#[derive(Debug, Clone, Default)]
pub(crate) struct ElementOverlayGeometry {
    pub prim_labels: Vec<([f32; 3], u32)>,
    pub vertex_labels: Vec<([f32; 3], u32)>,
    pub vertex_markers: Vec<crate::geometry::Vertex3D>,
    pub normals: Vec<crate::geometry::Vertex3D>,
}

/// The overlays of the other two element classes, read off the same scene
/// `Detail` as the points'. A PRIMITIVE's number stands at its centroid and
/// its normal is a whisker from there, `len` long. A VERTEX stands inset
/// from its point toward its primitive's centroid — its number, its marker
/// and the foot of its normal all there — so the vertices that share a
/// point are apart, each inside its own primitive.
///
/// A vertex's number is its index in the detail, which is its row in the
/// spreadsheet, as a point's is. A primitive's normal is Newell's, so a
/// quad that is not quite flat still has one; a primitive of fewer than
/// three points has a number and no normal. **A vertex's normal is its `N`
/// attribute where the detail carries one on its vertices, and its
/// primitive's normal where it does not** — which is what a vertex normal
/// is for: the normal of this corner of this face, where a point's is the
/// average over every face around it. So on a mesh with no vertex normals
/// the whiskers show the faceting, a fan of them at every shared point.
/// Each list of labels is capped as the points' is.
pub(crate) fn scene_element_overlays(
    geom: &crate::detail::Detail,
    want: ElementOverlays,
    len: f32,
) -> ElementOverlayGeometry {
    use glam::Vec3;
    const MAX_LABELS: usize = 2000;
    let mut out = ElementOverlayGeometry::default();
    if want == ElementOverlays::default() {
        return out;
    }
    let to_linear = |c: [u8; 3]| cce_ui::colors::to_linear_rgb(c.map(|c| c as f32 / 255.0));
    let (prim_color, vertex_color) = (to_linear(PRIM_LABEL_COLOR), to_linear(VERTEX_LABEL_COLOR));
    let own_normals = crate::geometry::own_vertex_normals(geom);
    let by_vertex = want.vertex_numbers || want.vertex_markers || want.vertex_normals;
    for prim in 0..geom.num_prims() {
        let pts = geom.prim_points(prim);
        if pts.is_empty() {
            continue;
        }
        let at: Vec<Vec3> = pts.iter().map(|&p| geom.pos(p as usize)).collect();
        let centroid = at.iter().copied().sum::<Vec3>() / at.len() as f32;
        let normal = if (want.prim_normals || want.vertex_normals) && at.len() >= 3 {
            let mut n = Vec3::ZERO;
            for (k, &a) in at.iter().enumerate() {
                let b = at[(k + 1) % at.len()];
                n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
            }
            n.normalize_or_zero()
        } else {
            Vec3::ZERO
        };
        let mut whisker = |from: Vec3, n: Vec3, len: f32, color: [f32; 3]| {
            if n != Vec3::ZERO {
                out.normals.push(crate::geometry::Vertex3D { position: from.to_array(), color });
                out.normals.push(crate::geometry::Vertex3D { position: (from + n * len).to_array(), color });
            }
        };
        if want.prim_normals {
            whisker(centroid, normal, len, prim_color);
        }
        if by_vertex {
            for (v, &p) in geom.prim_verts(prim).zip(&at) {
                let stands = p.lerp(centroid, VERTEX_LABEL_INSET);
                if want.vertex_normals {
                    let n = own_normals
                        .and_then(|n| n.get(v))
                        .map(|n| n.as_vec3().normalize_or_zero())
                        .filter(|n| *n != Vec3::ZERO)
                        .unwrap_or(normal);
                    whisker(stands, n, len * VERTEX_MARKER_SCALE, vertex_color);
                }
                if want.vertex_markers {
                    out.vertex_markers.push(crate::geometry::Vertex3D { position: stands.to_array(), color: [0.0; 3] });
                }
                if want.vertex_numbers && out.vertex_labels.len() < MAX_LABELS {
                    out.vertex_labels.push((stands.to_array(), v as u32));
                }
            }
        }
        if want.prim_numbers && out.prim_labels.len() < MAX_LABELS {
            out.prim_labels.push((centroid.to_array(), prim as u32));
        }
    }
    out
}

/// The scene's own edges as LINE_LIST pairs for the wire pass, carrying the
/// geometry's vertex colours.
///
/// The TOPOLOGICAL edge list, not the triangle soup's: an edge two faces
/// share is drawn once instead of twice, and a quad shows as a quad — the
/// fan diagonal was never an edge of the mesh, only of its triangulation.
/// The soup version was what the global Show Wireframe drew until
/// 2026-09-23, while the per-node meta Wireframe drew this one; with the
/// per-node flag retired there is one wireframe, and it is this one.
pub(crate) fn scene_edge_verts(geom: &crate::detail::Detail) -> Vec<crate::geometry::Vertex3D> {
    let mut wires = Vec::new();
    for e in geom.edges() {
        for &p in e {
            let p = p as usize;
            wires.push(crate::geometry::Vertex3D {
                position: geom.positions()[p],
                color: geom.color(p),
            });
        }
    }
    wires
}

/// A length for a status line: two decimals, without the zeros a whole
/// number of pixels would trail.
fn trim_number(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
