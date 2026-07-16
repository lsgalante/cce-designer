
use cce_ui::widget::TextLabel;
use cce_ui::colors;
use cce_ui::widget::WidgetHost;

use crate::app::{
    State, FsNode, WIDGET_COUNT,
    make_text_buffer, make_text_buffer_with_font,
    CONTENT_IDX, VIEWPORT_IDX, PARAM_IDX, PARAM_PLATE_IDX,
    BREADCRUMB_IDX, HEADER_IDX, RIGHT_MENUBAR_IDX,
    SPREADSHEET_MENUBAR_IDX, SPREADSHEET_IDX,
    MENUBAR_H,
    LEFT_MENUBAR_IDX, PARAM_MENUBAR_IDX, NETWORK_PANEL_IDX,
    push_circle_vertices, push_circle_border_vertices,
};
use cce_ui::engine::Vertex;
use crate::geometry::{
    network_sphere_vertices_with_errors,
};
use cce_ui::vk::TextSpan;
use cce_ui::engine::{
    push_widget_vertices, push_extra_quad_vertices,
    push_extra_quad_vertices_clipped, push_arc_background_vertices,
    push_plate_solid_border_vertices, push_rounded_rect_vertices_corners,
};

impl State {
    pub(crate) fn collect_vertices(&mut self, verts: &mut Vec<Vertex>) {
        verts.clear();
        let sw = self.width;
        let sh = self.height;

        let node_area_y = self.positions[CONTENT_IDX].1;
        let show_cursor = self.drag_widget.is_none() && self.app_drag.is_none();

        let clip = if self.circular_network_pane {
            (
                self.circular_network_layout.x - self.circular_network_layout.r,
                self.circular_network_layout.y - self.circular_network_layout.r,
                self.circular_network_layout.x + self.circular_network_layout.r,
                self.circular_network_layout.y + self.circular_network_layout.r,
            )
        } else {
            let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
            (cx, cy, cx + cw, cy + ch)
        };

        let clip_circle_val = if self.circular_network_pane {
            [self.circular_network_layout.x * self.scale as f32, self.circular_network_layout.y * self.scale as f32, self.circular_network_layout.r * self.scale as f32]
        } else {
            [0.0, 0.0, 0.0]
        };

        let mut draw_order: Vec<usize> = (0..WIDGET_COUNT).collect();
        draw_order.sort_by_key(|&i| {
            let base_key = if i == VIEWPORT_IDX || i == PARAM_PLATE_IDX || i == NETWORK_PANEL_IDX {
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
                self.slots.get_dyn_mut(i).z_index()
            };
            (self.has_any_open_menu(i), base_key)
        });

        // Root Backplate DISSOLVED (Phase 6as): it was a transparent draw-order shim —
        // register the visible widgets (registry consumers: coverage/parent walks) and
        // draw each top-level widget directly in the sorted order.
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

        let mut visited = vec![false; WIDGET_COUNT];
        for &i in &draw_order {
            if !self.slots.get_dyn_mut(i).visible() {
                continue;
            }
            unsafe {
                self.draw_element_recursive(
                    &*widget_ptrs[i],
                    verts,
                    sw,
                    sh,
                    [0.0, 0.0, 0.0],
                    show_cursor,
                    node_area_y,
                    &mut visited,
                    clip,
                    clip_circle_val,
                );
            }
        }

        self.push_context_border(verts, sw, sh, clip_circle_val);
    }

    /// Highlight border around the focused context's pane. The per-pane
    /// menubars are hidden in the floating layout, so this border is the
    /// only visual indicator of `focused_pane`.
    fn push_context_border(
        &self,
        verts: &mut Vec<Vertex>,
        sw: f32,
        sh: f32,
        clip_circle_val: [f32; 3],
    ) {
        if self.is_detached_network {
            return;
        }

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
                    push_circle_border_vertices(
                        self.circular_network_layout.x,
                        self.circular_network_layout.y,
                        self.circular_network_layout.r,
                        3.0,
                        sw,
                        sh,
                        color,
                        64,
                        clip_circle_val,
                        verts,
                    );
                    return;
                }
                color[3] *= self.network_opacity;
                self.positions[NETWORK_PANEL_IDX]
            }
            RIGHT_MENUBAR_IDX => {
                if !self.show_viewport {
                    return;
                }
                self.positions[VIEWPORT_IDX]
            }
            PARAM_MENUBAR_IDX => {
                if !self.show_parameters {
                    return;
                }
                self.positions[PARAM_IDX]
            }
            SPREADSHEET_MENUBAR_IDX => {
                if !self.show_spreadsheet {
                    return;
                }
                self.positions[SPREADSHEET_IDX]
            }
            _ => return,
        };

        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = cce_ui::layout::plate_corner_radius();
        let radii = cce_ui::widget::CornerRadii::new(r, r, r, r);
        push_plate_solid_border_vertices(
            x, y, w, h,
            radii,
            thickness,
            sw, sh,
            color,
            [0.0, 0.0, 0.0],
            verts,
        );
    }

    pub(crate) fn draw_widget_recursive(
        &self,
        idx: usize,
        verts: &mut Vec<Vertex>,
        sw: f32,
        sh: f32,
        clip: (f32, f32, f32, f32),
        clip_circle_val: [f32; 3],
        show_cursor: bool,
        node_area_y: f32,
        visited: &mut [bool],
    ) {
        if idx >= visited.len() {
            return;
        }
        if visited[idx] {
            return;
        }
        visited[idx] = true;

        let w = self.slots.get_dyn(idx);
        if !w.visible() {
            return;
        }

        let is_network_part = idx == CONTENT_IDX || idx == LEFT_MENUBAR_IDX || idx == BREADCRUMB_IDX || idx == NETWORK_PANEL_IDX;
        let active_clip_circle = if is_network_part { clip_circle_val } else { [0.0, 0.0, 0.0] };

        if idx == NETWORK_PANEL_IDX {
            if self.circular_network_pane {
                push_circle_vertices(
                    self.circular_network_layout.x,
                    self.circular_network_layout.y,
                    self.circular_network_layout.r,
                    sw,
                    sh,
                    w.color(),
                    64,
                    active_clip_circle,
                    verts,
                );
                push_circle_border_vertices(
                    self.circular_network_layout.x,
                    self.circular_network_layout.y,
                    self.circular_network_layout.r,
                    3.0,
                    sw,
                    sh,
                    [0.35, 0.65, 0.95, 0.80 * self.network_opacity],
                    64,
                    active_clip_circle,
                    verts,
                );
            } else {
                push_widget_vertices(w, sw, sh, active_clip_circle, verts);
            }
        } else if idx == CONTENT_IDX {
            if !self.circular_network_pane {
                push_widget_vertices(w, sw, sh, active_clip_circle, verts);
            }

            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                push_extra_quad_vertices_clipped(w, qx, qy, qw, qh, sw, sh, qc, clip, active_clip_circle, verts);
            }

            for (cx, cy, cr, cc) in w.extra_circles() {
                if cx >= clip.0 && cx <= clip.2 && cy >= clip.1 && cy <= clip.3 {
                    push_circle_vertices(cx, cy, cr, sw, sh, cc, 16, active_clip_circle, verts);
                }
            }

            if show_cursor {
                let px = self.positions[CONTENT_IDX].0;
                let py = self.positions[CONTENT_IDX].1;
                let cx = px + self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
                let cy = py + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
                let cw = self.grid_size_x;
                let ch = self.grid_size_y;
                let thickness = 2.0;
                let mut color = colors::highlight_primary_color();
                color[3] = 0.9;
                if self.graph().is_node_rect(cx, cy, cw, ch) {
                    let r = cce_ui::layout::graph_node_corner_radius();
                    let radii = cce_ui::widget::CornerRadii::new(r, r, r, r);
                    push_plate_solid_border_vertices(
                        cx, cy, cw, ch,
                        radii,
                        thickness,
                        sw, sh,
                        color,
                        active_clip_circle,
                        verts,
                    );
                } else {
                    push_extra_quad_vertices_clipped(w, cx, cy, cw, thickness, sw, sh, color, clip, active_clip_circle, verts);
                    push_extra_quad_vertices_clipped(w, cx, cy + ch - thickness, cw, thickness, sw, sh, color, clip, active_clip_circle, verts);
                    push_extra_quad_vertices_clipped(w, cx, cy + thickness, thickness, ch - thickness * 2.0, sw, sh, color, clip, active_clip_circle, verts);
                    push_extra_quad_vertices_clipped(w, cx + cw - thickness, cy + thickness, thickness, ch - thickness * 2.0, sw, sh, color, clip, active_clip_circle, verts);
                }
            }
        } else if idx == LEFT_MENUBAR_IDX && self.circular_network_pane {
            let cx = self.circular_network_layout.x;
            let cy = self.circular_network_layout.y;
            let r = self.circular_network_layout.r;

            let bg_color = w.color();
            push_arc_background_vertices(
                cx, cy, r,
                MENUBAR_H,
                std::f32::consts::PI,
                2.0 * std::f32::consts::PI,
                sw, sh,
                bg_color,
                64,
                active_clip_circle,
                verts,
            );

            let border_color = [0.22, 0.22, 0.28, 0.90 * self.network_opacity];
            push_arc_background_vertices(
                cx, cy, r - MENUBAR_H,
                1.5,
                std::f32::consts::PI,
                2.0 * std::f32::consts::PI,
                sw, sh,
                border_color,
                64,
                active_clip_circle,
                verts,
            );

            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                push_extra_quad_vertices(w, qx, qy, qw, qh, sw, sh, qc, active_clip_circle, verts);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                push_circle_vertices(cx, cy, cr, sw, sh, cc, 16, active_clip_circle, verts);
            }
            for (acx, acy, ar, ath, a_start, a_end, acolor) in w.extra_arcs() {
                push_arc_background_vertices(
                    acx, acy, ar,
                    ath,
                    a_start, a_end,
                    sw, sh,
                    acolor,
                    64,
                    active_clip_circle,
                    verts,
                );
            }
        } else {
            push_widget_vertices(w, sw, sh, active_clip_circle, verts);

            // The params pane serves its chrome through the legacy plain-quad view,
            // which carries flat quads only — the controls' rounded-rect backgrounds
            // (textbox/dropdown/button/checkbox/color) come from the rounded view,
            // drawn under the flat chrome and clipped to the pane's scroll viewport.
            if idx == PARAM_IDX {
                let (px, py, pw, ph) = self.positions[PARAM_IDX];
                let view = (px, py + 4.0, px + pw, py + ph - 4.0);
                let param_bg = self
                    .slots
                    .param
                    .as_any()
                    .downcast_ref::<cce_ui::widget::ParametersBg>()
                    .expect("PARAM_IDX must be a ParametersBg");
                for (qx, qy, qw, qh, qr, qc, corners) in param_bg.rounded_quads(&self.ui_context) {
                    let radii = cce_ui::widget::CornerRadii::new(
                        if corners.0 { qr } else { 0.0 },
                        if corners.1 { qr } else { 0.0 },
                        if corners.2 { qr } else { 0.0 },
                        if corners.3 { qr } else { 0.0 },
                    );
                    push_rounded_rect_vertices_corners(
                        qx, qy, qw, qh,
                        radii,
                        sw, sh,
                        qc,
                        active_clip_circle,
                        Some(view),
                        verts,
                    );
                }
            }

            for (qx, qy, qw, qh, qc) in w.extra_quads() {
                push_extra_quad_vertices(w, qx, qy, qw, qh, sw, sh, qc, active_clip_circle, verts);
            }
            for (cx, cy, cr, cc) in w.extra_circles() {
                push_circle_vertices(cx, cy, cr, sw, sh, cc, 16, active_clip_circle, verts);
            }
        }

        // Draw child elements recursively
        for child_ptr in self.ui_context.tree.children_ptrs(w.base().id()) {
            if let Some(child_idx) = self.find_widget_index(child_ptr as *const ()) {
                self.draw_widget_recursive(child_idx, verts, sw, sh, clip, clip_circle_val, show_cursor, node_area_y, visited);
            } else {
                unsafe {
                    self.draw_element_recursive(
                        &*child_ptr,
                        verts,
                        sw,
                        sh,
                        active_clip_circle,
                        show_cursor,
                        node_area_y,
                        visited,
                        clip,
                        clip_circle_val,
                    );
                }
            }
        }

        // Dropdown popover
        if w.visible() && (self.focused_widget == Some(idx) || idx == PARAM_IDX) {
            let mut popover_pc = cce_ui::layout::PopoverCollector::new();
            w.render_popover(&mut popover_pc);
            for (color, px, py, pw, ph) in popover_pc.rects {
                let ndc_x = (px / sw) * 2.0 - 1.0;
                let ndc_y = 1.0 - (py / sh) * 2.0;
                let ndc_w = (pw / sw) * 2.0;
                let ndc_h = (ph / sh) * 2.0;

                let v_tl = Vertex { position: [ndc_x, ndc_y], color, clip_circle: [0.0, 0.0, 0.0] };
                let v_tr = Vertex { position: [ndc_x + ndc_w, ndc_y], color, clip_circle: [0.0, 0.0, 0.0] };
                let v_bl = Vertex { position: [ndc_x, ndc_y - ndc_h], color, clip_circle: [0.0, 0.0, 0.0] };
                let v_br = Vertex { position: [ndc_x + ndc_w, ndc_y - ndc_h], color, clip_circle: [0.0, 0.0, 0.0] };

                verts.push(v_tl);
                verts.push(v_tr);
                verts.push(v_bl);

                verts.push(v_tr);
                verts.push(v_br);
                verts.push(v_bl);
            }
        }
    }

    fn draw_element_recursive(
        &self,
        element: &dyn WidgetHost,
        verts: &mut Vec<Vertex>,
        sw: f32,
        sh: f32,
        active_clip_circle: [f32; 3],
        show_cursor: bool,
        node_area_y: f32,
        visited: &mut [bool],
        clip: (f32, f32, f32, f32),
        clip_circle_val: [f32; 3],
    ) {
        if !element.visible() {
            return;
        }

        if let Some(idx) = self.find_widget_index(element as *const dyn WidgetHost as *const ()) {
            self.draw_widget_recursive(idx, verts, sw, sh, clip, clip_circle_val, show_cursor, node_area_y, visited);
            return;
        }

        push_widget_vertices(element, sw, sh, active_clip_circle, verts);

        for (qx, qy, qw, qh, qc) in element.extra_quads() {
            push_extra_quad_vertices(element, qx, qy, qw, qh, sw, sh, qc, active_clip_circle, verts);
        }
        for (cx, cy, cr, cc) in element.extra_circles() {
            push_circle_vertices(cx, cy, cr, sw, sh, cc, 16, active_clip_circle, verts);
        }

        for child_ptr in self.ui_context.tree.children_ptrs(element.base().id()) {
            unsafe {
                self.draw_element_recursive(
                    &*child_ptr,
                    verts,
                    sw,
                    sh,
                    active_clip_circle,
                    show_cursor,
                    node_area_y,
                    visited,
                    clip,
                    clip_circle_val,
                );
            }
        }
    }

    /// Rebuild the 2D vertex stream. The actual GPU upload happens in
    /// `VkRenderer::draw_frame`, which consumes `vertex_data` every frame.
    pub(crate) fn upload_vertices(&mut self) {
        self.text_dirty = true;
        let mut verts = std::mem::take(&mut self.vertex_data);
        self.collect_vertices(&mut verts);
        self.vertex_data = verts;
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
            self.text_dirty = true;
        }
    }

    pub(crate) fn prepare_text(&mut self, renderer: &mut cce_ui::vk::VkRenderer) {
        let mut current_popovers = Vec::new();
        {
            fn collect_popovers(
                w: &dyn WidgetHost,
                popovers: &mut Vec<(f32, f32, f32, f32)>,
                ctx: &cce_ui::context::UiContext,
            ) {
                if let Some(rect) = w.popover_rect() {
                    popovers.push(rect);
                }
                for child_ptr in ctx.tree.children_ptrs(w.base().id()) {
                    unsafe {
                        if let Some(child) = child_ptr.as_ref() {
                            collect_popovers(child, popovers, ctx);
                        }
                    }
                }
            }

            for i in 0..WIDGET_COUNT {
                let w = self.slots.get_dyn(i);
                if w.visible() {
                    collect_popovers(w, &mut current_popovers, &self.ui_context);
                }
            }
        }

        if current_popovers != self.last_popover_rects {
            self.last_popover_rects = current_popovers;
            self.text_dirty = true;
        }

        if !self.text_dirty {
            return;
        }
        self.text_dirty = false;

        // 1. Prepare text on all widgets using self.font_system
        for i in 0..WIDGET_COUNT {
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if !is_menubar {
                self.slots.get_dyn_mut(i).prepare_text(&mut self.font_system);
            }
        }

        // 2. Destructure self
        let sw = self.width;
        let sh = self.height;
        let circular_network_pane = self.circular_network_pane;
        let network_circle_x = self.circular_network_layout.x;
        let network_circle_y = self.circular_network_layout.y;
        let network_circle_radius = self.circular_network_layout.r;
        let network_opacity = self.network_opacity;
        let focused_widget = self.focused_widget;
        let _ = sw;
        let _ = sh;

        let Self {
            ref mut font_system,
            ref mut swash_cache,
            physical_width, physical_height, scale,
            ref slots,
            ref mut text_buffer_cache,
            ref ui_context,
            ref positions,
            ..
        } = *self;

        // Clear cache if too large to prevent unbounded memory growth
        if text_buffer_cache.len() > 500 {
            text_buffer_cache.clear();
        }

        // Walk-derived widget text (RFC Phase 6ap): each non-menubar widget's subtree text
        // as prims — (text, x, y, size, color, font, clip bounds), the walk's per-widget
        // content font and container clips composed in — replacing the legacy
        // get_text_items / text_labels_with_font_and_bounds getters. Shaping stays
        // app-side in text_buffer_cache (same size*1.4 metrics as before).
        let mut widget_text: Vec<Vec<(String, f32, f32, f32, [u8; 3], Option<String>, Option<[f32; 4]>)>> =
            Vec::with_capacity(WIDGET_COUNT);
        for i in 0..WIDGET_COUNT {
            let w = slots.get_dyn(i);
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if !w.visible() || is_menubar {
                widget_text.push(Vec::new());
                continue;
            }
            let mut scratch = cce_ui::scene::paint::PaintCtx::new();
            cce_ui::scene::painter::append_widget_text(ui_context, w, &mut scratch);
            widget_text.push(
                scratch
                    .finish()
                    .items
                    .into_iter()
                    .filter_map(|item| match item.prim {
                        cce_ui::scene::paint::Prim::Text { text, x, y, font_size, color, font, bounds, .. } =>
                            Some((text, x, y, font_size, color, font, bounds)),
                        _ => None,
                    })
                    .collect(),
            );
        }

        // Pass 1: Populate text_buffer_cache with shaped buffers
        for (i, texts) in widget_text.iter().enumerate() {
            for (text, x, y, font_size, _color, font, _bounds) in texts {
                let mut is_curved = false;
                if circular_network_pane && i == LEFT_MENUBAR_IDX && text.chars().count() == 1 {
                    let dx = x - network_circle_x;
                    let dy = y - network_circle_y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= network_circle_radius - 35.0 && dist <= network_circle_radius + 5.0 {
                        is_curved = true;
                    }
                }

                if is_curved {
                    let key = (text.clone(), (12.0 * 100.0) as u32, None);
                    if !text_buffer_cache.contains_key(&key) {
                        let buf = make_text_buffer(font_system, text, 12.0);
                        text_buffer_cache.insert(key, buf);
                    }
                } else {
                    let key = (text.clone(), (font_size * 100.0) as u32, font.clone());
                    if !text_buffer_cache.contains_key(&key) {
                        let buf = make_text_buffer_with_font(font_system, text, *font_size, font.as_deref());
                        text_buffer_cache.insert(key, buf);
                    }
                }
            }
        }

        // Pass 1b: Populate text_buffer_cache with popover texts
        for i in 0..WIDGET_COUNT {
            let w = slots.get_dyn(i);
            if !w.visible() {
                continue;
            }
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if is_menubar {
                continue;
            }
            if focused_widget == Some(i) || i == PARAM_IDX {
                let mut popover_pc = cce_ui::layout::PopoverCollector::new();
                w.render_popover(&mut popover_pc);
                for (t, size, _x, _y, _tc, font_opt, _bounds) in popover_pc.texts {
                    let key = (t.clone(), (size * 100.0) as u32, font_opt.clone());
                    if !text_buffer_cache.contains_key(&key) {
                        let buf = make_text_buffer_with_font(font_system, &t, size, font_opt.as_deref());
                        text_buffer_cache.insert(key, buf);
                    }
                }
            }
        }

        let s = scale as f32;

        // Build the frame's spans against the shaped cache. From here the cache
        // is only read (`get`), so the borrows stay immutable for the spans.
        let text_buffer_cache = &*text_buffer_cache;
        let mut spans: Vec<TextSpan> = Vec::new();

        for i in 0..WIDGET_COUNT {
            let w = slots.get_dyn(i);
            if !w.visible() {
                continue;
            }
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if is_menubar {
                continue;
            }
            let is_node = i == CONTENT_IDX;
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;

            // Widget-level clip bounds (physical px), matching the old TextBounds.
            let bounds = if is_node {
                if circular_network_pane {
                    [
                        ((network_circle_x - network_circle_radius) * s) as i32,
                        ((network_circle_y - network_circle_radius) * s) as i32,
                        ((network_circle_x + network_circle_radius) * s) as i32,
                        (((network_circle_y + network_circle_radius) * s) as i32).max(0),
                    ]
                } else {
                    let (gx, gy, gw, gh) = positions[CONTENT_IDX];
                    [
                        (gx * s) as i32,
                        (gy * s) as i32,
                        ((gx + gw) * s) as i32,
                        (((gy + gh) * s) as i32).max(0),
                    ]
                }
            } else {
                [0, 0, physical_width as i32, physical_height as i32]
            };

            for (text, x, y, font_size, color, font, label_bounds) in &widget_text[i] {
                let mut is_curved = false;
                if circular_network_pane && i == LEFT_MENUBAR_IDX && text.chars().count() == 1 {
                    let dx = x - network_circle_x;
                    let dy = y - network_circle_y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= network_circle_radius - 35.0 && dist <= network_circle_radius + 5.0 {
                        is_curved = true;
                    }
                }

                if is_curved {
                    // Curved rim label: rotate the glyph quad about its center to
                    // face outward, clipped to the circular pane — the ash port of
                    // the old render-to-texture + TexturedVertex path.
                    let font_size = 12.0;
                    let tw = TextLabel::estimate_width(text, font_size);
                    let th = font_size;
                    let cx = x + tw / 2.0;
                    let cy = y + th / 2.0;
                    let theta = (cy - network_circle_y).atan2(cx - network_circle_x);
                    let angle = theta + std::f32::consts::FRAC_PI_2;
                    let key = (text.clone(), (font_size * 100.0) as u32, None);
                    if let Some(buf) = text_buffer_cache.get(&key) {
                        spans.push(TextSpan {
                            buffer: buf,
                            left: (x * s).round(),
                            top: (y * s).round(),
                            scale: s,
                            bounds: None,
                            default_color: [
                                color[0] as f32 / 255.0,
                                color[1] as f32 / 255.0,
                                color[2] as f32 / 255.0,
                                1.0,
                            ],
                            rotation: Some((angle, cx * s, cy * s)),
                            clip_circle: [
                                network_circle_x * s,
                                network_circle_y * s,
                                network_circle_radius * s,
                            ],
                        });
                    }
                } else {
                    if circular_network_pane && is_network_part && i != LEFT_MENUBAR_IDX {
                        let dx = x - network_circle_x;
                        let dy = y - network_circle_y;
                        let dist_sq = dx * dx + dy * dy;
                        if dist_sq > network_circle_radius * network_circle_radius {
                            continue;
                        }
                    }
                    let mut item_bounds = bounds;
                    if let Some([l, t, r, b]) = label_bounds {
                        let pl = (l * s).round() as i32;
                        let pt = (t * s).round() as i32;
                        let pr = (r * s).round() as i32;
                        let pb = (b * s).round() as i32;
                        item_bounds = [
                            item_bounds[0].max(pl),
                            item_bounds[1].max(pt),
                            item_bounds[2].min(pr),
                            item_bounds[3].min(pb),
                        ];
                    }
                    let key = (text.clone(), (font_size * 100.0) as u32, font.clone());
                    let Some(buf) = text_buffer_cache.get(&key) else { continue };
                    let alpha = if is_network_part { network_opacity.clamp(0.0, 1.0) } else { 1.0 };
                    spans.push(TextSpan {
                        buffer: buf,
                        left: (x * s).round(),
                        top: (y * s).round(),
                        scale: s,
                        bounds: Some(item_bounds),
                        default_color: [
                            color[0] as f32 / 255.0,
                            color[1] as f32 / 255.0,
                            color[2] as f32 / 255.0,
                            alpha,
                        ],
                        rotation: None,
                        clip_circle: [0.0; 3],
                    });
                }
            }
        }

        // Popover text spans
        for i in 0..WIDGET_COUNT {
            let w = slots.get_dyn(i);
            if !w.visible() {
                continue;
            }
            let is_menubar = i == HEADER_IDX || i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX;
            if is_menubar {
                continue;
            }
            if focused_widget == Some(i) || i == PARAM_IDX {
                let mut popover_pc = cce_ui::layout::PopoverCollector::new();
                w.render_popover(&mut popover_pc);
                for (t, size, x, y, tc, font_opt, label_bounds) in popover_pc.texts {
                    let key = (t.clone(), (size * 100.0) as u32, font_opt.clone());
                    if let Some(buf) = text_buffer_cache.get(&key) {
                        let mut item_bounds = [0, 0, physical_width as i32, physical_height as i32];
                        if let Some([l, t_bound, r, b]) = label_bounds {
                            let pl = (l * s).round() as i32;
                            let pt = (t_bound * s).round() as i32;
                            let pr = (r * s).round() as i32;
                            let pb = (b * s).round() as i32;
                            item_bounds = [
                                item_bounds[0].max(pl),
                                item_bounds[1].max(pt),
                                item_bounds[2].min(pr),
                                item_bounds[3].min(pb),
                            ];
                        }
                        spans.push(TextSpan {
                            buffer: buf,
                            left: (x * s).round(),
                            top: (y * s).round(),
                            scale: s,
                            bounds: Some(item_bounds),
                            default_color: [tc[0], tc[1], tc[2], 1.0],
                            rotation: None,
                            clip_circle: [0.0; 3],
                        });
                    }
                }
            }
        }

        renderer.prepare_text(font_system, swash_cache, &spans);
    }

}
