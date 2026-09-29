//! Working with 2D images from the viewport and the palette: the commands
//! that turn the camera to an image, and the ones that make an image and put
//! shapes and text on it.
//!
//! An image is a page (`src/page.rs`) — a `page` node and whatever draws on
//! it — and the viewport shows the displayed one as a quad standing in the
//! scene. Everything here is a registry command, so each is in the palette,
//! bindable and scriptable over MCP's `run_command`.

use crate::app::{FsNode, State};
use crate::page::{is_page_node, PageShown};
use glam::Vec3;

/// What an Add … to Image command puts on the image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageLayer {
    Rectangle,
    Ellipse,
    Line,
    Polygon,
    Text,
}

impl ImageLayer {
    fn template(self) -> &'static str {
        match self {
            ImageLayer::Text => "Page Text",
            _ => "Page Shape",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ImageLayer::Rectangle => "Rectangle",
            ImageLayer::Ellipse => "Ellipse",
            ImageLayer::Line => "Line",
            ImageLayer::Polygon => "Polygon",
            ImageLayer::Text => "Text",
        }
    }
}

/// The vertical field of view `Viewport3D::get_matrices` projects with,
/// halved.
const HALF_FOV: f32 = 0.45;

/// How much of the pane a framed image leaves as margin, as a factor on the
/// distance: the image spans ~93% of the axis that binds.
const FRAME_MARGIN: f32 = 1.08;

/// The distance at which an image `size` world units across fills a pane of
/// `aspect`, seen head-on. The projection is a perspective, and a plane
/// square to the view axis is scaled by it and not distorted, so one distance
/// fits both axes and the nearer fit of the two is the one that binds.
pub fn fit_distance(size: [f32; 2], aspect: f32) -> f32 {
    let t = HALF_FOV.tan();
    let by_height = size[1] / (2.0 * t);
    let by_width = size[0] / (2.0 * t * aspect.max(1e-4));
    by_height.max(by_width) * FRAME_MARGIN
}

/// The distance at which one pixel of an image `image_px` tall and `height`
/// world units tall covers one pixel of a pane `pane_px` tall.
pub fn pixel_distance(height: f32, image_px: u32, pane_px: u32) -> f32 {
    let world_per_px = height / image_px.max(1) as f32;
    world_per_px * pane_px.max(1) as f32 / (2.0 * HALF_FOV.tan())
}

fn set_param(node: &mut FsNode, name: &str, value: String) {
    if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
        p.set_text(value);
    }
}

/// A length for a parameter row: what the page's unit is read to. Pixels
/// are whole, the rest two decimals.
fn row_number(v: f32, unit: crate::page::PageUnit) -> String {
    if unit == crate::page::PageUnit::Pixels {
        format!("{}", v.round())
    } else {
        format!("{v:.2}")
    }
}

impl State {
    /// The pane's aspect as the stage pass has it.
    fn viewport_aspect(&self) -> f32 {
        if self.square_viewport {
            1.0
        } else {
            self.last_viewport_width.max(1) as f32 / self.last_viewport_height.max(1) as f32
        }
    }

    /// Put the active camera square to the image's plane, `dist` out from
    /// `center` along +Z — the side an image faces.
    ///
    /// The Default Camera has a fixed base ray and an orbit over it, so the
    /// orbit is set to what cancels the ray's own yaw and pitch and the zoom
    /// to what makes the distance. A camera node is rewritten — Position,
    /// Pivot and Rotation — as Frame All rewrites one; its Rotation takes up
    /// whatever orbit the viewport widget is holding, which `get_matrices`
    /// applies to every camera. Returns the distance reached, which is short
    /// of the one asked for where the zoom's clamp refuses it.
    fn face_image(&mut self, center: Vec3, dist: f32) -> f32 {
        let camera_name = self.active_camera.clone();
        let (orbit_x, orbit_y) = (self.viewport().rotation_x, self.viewport().rotation_y);
        let mut reached = None;
        if camera_name != "Default Camera" {
            let dir = self.current_dir_mut();
            if let Some(node) =
                dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name)
            {
                let fmt3 = |v: Vec3| format!("{:.3}:{:.3}:{:.3}", v.x, v.y, v.z);
                set_param(node, "Pivot", fmt3(center));
                set_param(node, "Position", fmt3(center + Vec3::Z * dist));
                set_param(
                    node,
                    "Rotation",
                    format!("{:.3}:{:.3}:0.000", orbit_x.to_degrees(), orbit_y.to_degrees()),
                );
                reached = Some(dist);
            }
        }
        let reached = match reached {
            Some(d) => {
                let vp = self.viewport_mut();
                vp.zoom = 1.0;
                vp.pending_yaw = 0.0;
                vp.pending_pitch = 0.0;
                vp.reset_velocity();
                d
            }
            None => {
                let base = Vec3::new(2.5, 1.8, 2.5);
                let vp = self.viewport_mut();
                vp.pivot = center;
                vp.rotation_y = base.x.atan2(base.z);
                vp.rotation_x = (base.y / base.length()).asin();
                vp.zoom = (dist / base.length())
                    .clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
                vp.reset_velocity();
                vp.zoom * base.length()
            }
        };
        self.viewport_dirty = true;
        self.sync_nodes();
        self.sync_parameters_pane();
        reached
    }

    /// The image the viewport shows, or a status line saying there is none.
    fn image_to_frame(&mut self) -> Option<PageShown> {
        let shown = self.page_shown.clone();
        if shown.is_none() {
            self.update_status_text("No image is shown: turn on an image node's display flag.");
        }
        shown
    }

    /// `Frame Image`: turn the camera square to the image and fit it to the
    /// pane.
    pub fn frame_image(&mut self) -> bool {
        let Some(shown) = self.image_to_frame() else { return false };
        let size = shown.world_size(self.world_unit_mm());
        let dist = fit_distance(size, self.viewport_aspect());
        let reached = self.face_image(Vec3::from_array(shown.origin), dist);
        if (reached - dist).abs() > dist * 1e-3 {
            self.update_status_text("Framed the image as far as the zoom goes.");
        } else {
            self.update_status_text("Framed the image.");
        }
        true
    }

    /// `View Image Pixels 1:1`: turn the camera square to the image at the
    /// distance where one of its pixels covers one of the display's.
    pub fn view_image_pixels(&mut self) -> bool {
        let Some(shown) = self.image_to_frame() else { return false };
        let size = shown.world_size(self.world_unit_mm());
        let dist = pixel_distance(size[1], shown.pixels.1, self.last_viewport_height);
        let reached = self.face_image(Vec3::from_array(shown.origin), dist);
        if (reached - dist).abs() > dist * 1e-3 {
            self.update_status_text("Image pixels 1:1 is past what the zoom reaches.");
        } else {
            self.update_status_text("Image pixels 1:1.");
        }
        true
    }

    /// The scene's image in world space, for a bound that has to hold it.
    pub(crate) fn image_world_corners(&self) -> Option<[[f32; 3]; 4]> {
        let unit = self.world_unit_mm();
        self.page_shown.as_ref().map(|s| s.world_corners(unit))
    }

    /// Add a node from a template to the current level, at the nearest free
    /// cell to `at`, and return its slot. The display flag is left as the
    /// template has it cleared, for the caller to decide.
    fn add_template_node(&mut self, template: &str, at: (f32, f32)) -> Option<usize> {
        let idx = self.node_templates.iter().position(|t| {
            t.label.eq_ignore_ascii_case(template) || t.node.name.eq_ignore_ascii_case(template)
        })?;
        let mut node = self.node_templates[idx].node.clone();
        crate::app::regenerate_node_ids(&mut node);
        node.position = self.find_empty_cell(at.0, at.1, None);
        node.name = self.get_lowest_unused_name(&node.name);
        node.geometry_visible = false;
        self.current_dir_mut().children.push(node);
        Some(self.current_dir().children.len() - 1)
    }

    /// Show `slot`, select it, and bring everything that reads the tree up
    /// to date — the tail every command here ends on.
    fn show_and_select(&mut self, slot: usize) {
        self.current_dir_mut().set_child_geometry_visible(slot, true);
        if let Some(cell) = self.current_dir().children.get(slot).map(|n| n.position) {
            self.grid_cursor_col = cell.0 as i32;
            self.grid_cursor_row = cell.1 as i32;
        }
        self.deselected_cell = None;
        self.sync_nodes();
        self.graph_mut().set_selected_node(Some(slot));
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
    }

    /// `New Image`: a page node at the grid cursor, shown and selected.
    pub fn new_image(&mut self) -> Option<usize> {
        let at = (self.grid_cursor_col as f32, self.grid_cursor_row as f32);
        let slot = self.add_template_node("Page", at)?;
        self.show_and_select(slot);
        Some(slot)
    }

    /// The node an Add … to Image command draws on: the selected node when
    /// it belongs to an image, else the image the level shows.
    fn image_target(&self) -> Option<usize> {
        let dir = self.current_dir();
        let is_image = |slot: &usize| {
            dir.children.get(*slot).is_some_and(|n| is_page_node(&n.node_type))
        };
        self.selected_slots().into_iter().find(is_image).or_else(|| {
            dir.children
                .iter()
                .enumerate()
                .filter(|(_, c)| is_page_node(&c.node_type) && c.geometry_visible)
                .map(|(i, _)| i)
                .next_back()
        })
    }

    /// `Add Rectangle to Image` and its family: a shape or text node wired
    /// after the image's node, sized and placed from the image itself, shown
    /// and selected so its rows are what the params pane holds next.
    ///
    /// With no image at the level one is made first. A target in the MIDDLE
    /// of a chain has the new node inserted after it: what read the target
    /// reads the new node.
    pub fn add_to_image(&mut self, layer: ImageLayer) -> Option<usize> {
        let target = match self.image_target() {
            Some(slot) => slot,
            None => self.new_image()?,
        };
        let (target_name, target_pos) = {
            let n = &self.current_dir().children[target];
            (n.name.clone(), n.position)
        };
        let Some(page) = crate::page::resolve_page(
            &self.fs_root,
            &self.current_dir().children[target],
            &mut Vec::new(),
        ) else {
            self.update_status_text(&format!("{target_name} draws on no image: wire it to one."));
            return None;
        };

        let slot = self.add_template_node(layer.template(), (target_pos.0, target_pos.1 + 1.0))?;
        let new_name = self.current_dir().children[slot].name.clone();
        for (i, sibling) in self.current_dir_mut().children.iter_mut().enumerate() {
            if i == slot || !is_page_node(&sibling.node_type) && sibling.node_type != "export" {
                continue;
            }
            for p in sibling.params.iter_mut() {
                if p.name == "Input" && p.text().trim() == target_name {
                    p.set_text(new_name.clone());
                }
            }
        }

        // In the page's unit, from the page's size: the middle of the image,
        // a third of its width, and type a twentieth of its height tall.
        let unit = page.unit;
        let (w, h) = (page.in_unit(page.size[0]), page.in_unit(page.size[1]));
        let num = |v: f32| row_number(v, unit);
        let node = &mut self.current_dir_mut().children[slot];
        set_param(node, "Input", target_name);
        set_param(node, "X", num(w * 0.5));
        set_param(node, "Y", num(h * 0.5));
        match layer {
            ImageLayer::Text => {
                set_param(node, "Text", "Text".to_string());
                set_param(node, "Size", num((h * 0.05).max(page.in_unit(1.0 / page.scale()))));
                set_param(node, "Vertical", "Middle".to_string());
            }
            shape => {
                let side = w.min(h) / 3.0;
                set_param(node, "Shape", shape.label().to_string());
                set_param(node, "Width", num(if shape == ImageLayer::Line { w / 3.0 } else { side }));
                set_param(node, "Height", num(side));
                // A hairline that survives the raster: a four-hundredth of
                // the short side, and never under a pixel and a half.
                let stroke = (page.size[0].min(page.size[1]) / 400.0).max(1.5 / page.scale());
                let stroke = page.in_unit(stroke);
                let stroke = if unit == crate::page::PageUnit::Pixels {
                    format!("{}", stroke.round().max(1.0))
                } else {
                    format!("{stroke:.3}")
                };
                set_param(node, "Stroke Width", stroke);
            }
        }
        self.show_and_select(slot);
        self.update_status_text(&format!("Added {} to {}.", layer.label().to_lowercase(), new_name));
        Some(slot)
    }
}
