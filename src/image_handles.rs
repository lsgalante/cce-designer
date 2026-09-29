//! The viewer states of the 2D context: a shape and a run of text on an
//! image, placed by dragging where the image stands in the viewport.
//!
//! The third and fourth [`HandleSource`]s, and the first whose handles are
//! not where their parameters say. A shape's rows are in the IMAGE'S unit
//! from the image's top-left corner, y running down; its handles are world
//! positions on a quad standing in the scene. [`PageFrame`] is the
//! conversion, and the framework hands it over in the [`HandleCtx`], read
//! off the `page` node up the chain without composing anything.
//!
//! They are also the first whose handles hang off one another, which is
//! what [`HandleSource::drag`] is for:
//!
//! - **move** — the centre. The other handles go with it: a shape dragged
//!   by its middle is moved, not resized.
//! - **size** — a corner of the box, in the shape's own turned frame. The
//!   box grows about its centre, so the opposite corner moves the other way.
//! - **turn** — the middle of the box's right edge. Its DIRECTION from the
//!   centre is the rotation; the corner is carried round with it.
//!
//! A line has two: its middle, and an end that sets its length and its
//! angle together, which is how a line is drawn.
//!
//! Every drag follows the cursor's ray to the image's plane
//! ([`HandleSource::plane`]), so a handle stays under the pointer from any
//! orbit, and not only while the view is square to the image.

use crate::app::FsNode;
use crate::geometry::{node_param_f32, node_param_str};
use crate::page::PageFrame;
use crate::viewer_state::{HandleCtx, HandleSource};
use glam::Vec3;

pub struct ShapeHandles;
pub struct TextHandles;

/// `v` turned by `degrees`, in the page's frame: y runs down, so a positive
/// angle is clockwise as the page is looked at — the way `Page::shape`
/// turns the shape itself.
fn turned(degrees: f32, v: [f32; 2]) -> [f32; 2] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [v[0] * cos - v[1] * sin, v[0] * sin + v[1] * cos]
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn length(v: [f32; 2]) -> f32 {
    v[0].hypot(v[1])
}

/// The angle of `v` in degrees, or None for a vector too short to have one.
fn angle_of(v: [f32; 2], frame: &PageFrame) -> Option<f32> {
    (length(v) > one_pixel(frame) * 0.5).then(|| v[1].atan2(v[0]).to_degrees())
}

/// One pixel of the raster, in the page's unit: the least a size can be.
fn one_pixel(frame: &PageFrame) -> f32 {
    frame.unit.from_inches(1.0 / frame.scale(), frame.scale())
}

fn set(node: &mut FsNode, name: &str, value: String) {
    if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
        p.set_text(value);
    }
}

fn is_line(node: &FsNode) -> bool {
    node_param_str(node, "Shape", "Rectangle").eq_ignore_ascii_case("Line")
}

/// The shape's centre, box and turn, as its rows have them.
fn shape_rows(node: &FsNode) -> ([f32; 2], [f32; 2], f32) {
    (
        [node_param_f32(node, "X", 0.0), node_param_f32(node, "Y", 0.0)],
        [node_param_f32(node, "Width", 1.0).abs(), node_param_f32(node, "Height", 1.0).abs()],
        node_param_f32(node, "Rotation", 0.0),
    )
}

impl HandleSource for ShapeHandles {
    fn name(&self) -> &'static str {
        "Image Shape"
    }

    fn accepts(&self, node_type: &str) -> bool {
        node_type.eq_ignore_ascii_case("page_shape")
    }

    fn read(&self, node: &FsNode, ctx: &HandleCtx) -> Vec<Vec3> {
        let Some(frame) = ctx.page else { return Vec::new() };
        let (c, size, turn) = shape_rows(node);
        let world = |at: [f32; 2]| frame.to_world(at, ctx.world_unit_mm);
        let edge = add(c, turned(turn, [size[0] * 0.5, 0.0]));
        if is_line(node) {
            vec![world(c), world(edge)]
        } else {
            let corner = add(c, turned(turn, [size[0] * 0.5, size[1] * 0.5]));
            vec![world(c), world(corner), world(edge)]
        }
    }

    fn write(&self, node: &mut FsNode, handles: &[Vec3], ctx: &HandleCtx) {
        let Some(frame) = ctx.page else { return };
        let page = |p: &Vec3| frame.from_world(*p, ctx.world_unit_mm);
        let least = one_pixel(&frame);
        let kept = node_param_f32(node, "Rotation", 0.0);
        let (c, size, turn) = match handles {
            [c, end] => {
                let (c, along) = (page(c), sub(page(end), page(c)));
                let turn = angle_of(along, &frame).unwrap_or(kept);
                (c, [length(along) * 2.0, node_param_f32(node, "Height", 1.0)], turn)
            }
            [c, corner, edge] => {
                let c = page(c);
                let turn = angle_of(sub(page(edge), c), &frame).unwrap_or(kept);
                // The corner in the shape's own frame: the page turned back.
                let half = turned(-turn, sub(page(corner), c));
                (c, [half[0].abs() * 2.0, half[1].abs() * 2.0], turn)
            }
            _ => return,
        };
        set(node, "X", frame.row(c[0]));
        set(node, "Y", frame.row(c[1]));
        set(node, "Width", frame.row(size[0].max(least)));
        if handles.len() == 3 {
            set(node, "Height", frame.row(size[1].max(least)));
        }
        set(node, "Rotation", format!("{turn:.1}"));
    }

    fn drag(&self, handles: &mut Vec<Vec3>, moved: usize, to: Vec3, ctx: &HandleCtx) {
        let Some(frame) = ctx.page else {
            handles[moved] = to;
            return;
        };
        let page = |p: Vec3| frame.from_world(p, ctx.world_unit_mm);
        let world = |at: [f32; 2]| frame.to_world(at, ctx.world_unit_mm);
        match (moved, handles.len()) {
            // The middle carries the rest.
            (0, _) => {
                let by = to - handles[0];
                for h in handles.iter_mut() {
                    *h += by;
                }
            }
            // The corner sets the box; the turn handle sits on the box's
            // edge, so it follows the width.
            (1, 3) => {
                let c = page(handles[0]);
                let turn = angle_of(sub(page(handles[2]), c), &frame).unwrap_or(0.0);
                let half = turned(-turn, sub(page(to), c));
                handles[1] = to;
                handles[2] = world(add(c, turned(turn, [half[0].abs(), 0.0])));
            }
            // The turn handle carries the corner round the middle.
            (2, 3) => {
                let c = page(handles[0]);
                let was = angle_of(sub(page(handles[2]), c), &frame);
                let now = angle_of(sub(page(to), c), &frame);
                if let (Some(was), Some(now)) = (was, now) {
                    let corner = sub(page(handles[1]), c);
                    handles[1] = world(add(c, turned(now - was, corner)));
                }
                handles[2] = to;
            }
            _ => handles[moved] = to,
        }
    }

    fn plane(&self, ctx: &HandleCtx) -> Option<(Vec3, Vec3)> {
        ctx.page.map(|frame| (Vec3::from_array(frame.origin), Vec3::Z))
    }

    fn outline(&self, node: &FsNode, ctx: &HandleCtx) -> Vec<Vec3> {
        let Some(frame) = ctx.page else { return Vec::new() };
        let (c, size, turn) = shape_rows(node);
        let (hw, hh) = (size[0] * 0.5, size[1] * 0.5);
        let corners: &[[f32; 2]] = if is_line(node) {
            &[[-hw, 0.0], [hw, 0.0]]
        } else {
            &[[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
        };
        corners
            .iter()
            .map(|v| frame.to_world(add(c, turned(turn, *v)), ctx.world_unit_mm))
            .collect()
    }

    fn cage(&self) -> bool {
        false
    }

    fn extensible(&self) -> bool {
        false
    }

    fn hints(&self) -> &'static str {
        "drag the middle to move, the corner to size, the edge to turn"
    }

    fn handle_label(&self, i: usize) -> String {
        ["move", "size", "turn"].get(i).copied().unwrap_or("").to_string()
    }
}

impl HandleSource for TextHandles {
    fn name(&self) -> &'static str {
        "Image Text"
    }

    fn accepts(&self, node_type: &str) -> bool {
        node_type.eq_ignore_ascii_case("page_text")
    }

    /// The anchor — what the text is aligned against — and a handle one
    /// Size below it, so the type's size is a length that can be seen.
    fn read(&self, node: &FsNode, ctx: &HandleCtx) -> Vec<Vec3> {
        let Some(frame) = ctx.page else { return Vec::new() };
        let at = [node_param_f32(node, "X", 0.0), node_param_f32(node, "Y", 0.0)];
        let size = node_param_f32(node, "Size", 0.25).abs();
        vec![
            frame.to_world(at, ctx.world_unit_mm),
            frame.to_world(add(at, [0.0, size]), ctx.world_unit_mm),
        ]
    }

    fn write(&self, node: &mut FsNode, handles: &[Vec3], ctx: &HandleCtx) {
        let Some(frame) = ctx.page else { return };
        let [at, below] = handles else { return };
        let at = frame.from_world(*at, ctx.world_unit_mm);
        let size = length(sub(frame.from_world(*below, ctx.world_unit_mm), at));
        set(node, "X", frame.row(at[0]));
        set(node, "Y", frame.row(at[1]));
        set(node, "Size", frame.row(size.max(one_pixel(&frame))));
    }

    fn drag(&self, handles: &mut Vec<Vec3>, moved: usize, to: Vec3, _ctx: &HandleCtx) {
        if moved == 0 {
            let by = to - handles[0];
            for h in handles.iter_mut() {
                *h += by;
            }
        } else {
            handles[moved] = to;
        }
    }

    fn plane(&self, ctx: &HandleCtx) -> Option<(Vec3, Vec3)> {
        ctx.page.map(|frame| (Vec3::from_array(frame.origin), Vec3::Z))
    }

    fn extensible(&self) -> bool {
        false
    }

    fn hints(&self) -> &'static str {
        "drag the anchor to move, the handle under it to size"
    }

    fn handle_label(&self, i: usize) -> String {
        if i == 0 { "move".into() } else { "size".into() }
    }
}
