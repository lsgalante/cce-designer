//! The curve viewer state: an interactive viewport tool for the native
//! `curve` node, entered from the node's context menu ("Edit Points").
//!
//! While active, the node's control points draw as screen-space handles
//! projected through the raster scene's cached mvp — the same path the meta
//! Point Numbers overlay rides — and the pointer edits them in place:
//!
//! - **left press on a handle** grabs it; dragging moves the point on the
//!   camera-facing plane at its own depth (the grab keeps the point's NDC z,
//!   so orbiting between edits never makes a drag jump);
//! - **left press on empty viewport space** appends a new point there, at the
//!   depth of the last control point, and immediately drags it;
//! - **right press on a handle** deletes that point; Delete/Backspace deletes
//!   the selected (last-clicked) one;
//! - **Escape** exits the state.
//!
//! Edits write the node's "Points" param through the same resync sequence as
//! `McpAction::SetParam`, so the params pane, spreadsheet, and scene all
//! follow live. The tool holds the node's *id*, not its slot: renames and
//! graph edits don't detach it, and if the node disappears (deleted, project
//! reloaded) every handler resolves nothing and the tool drops out lazily.

use crate::app::{FsNode, State};
use crate::geometry::{format_curve_points, node_param_str, parse_curve_points};
use glam::{Mat4, Vec3, Vec4};

/// How close (logical px) a press must land to a projected handle to grab it.
pub const HANDLE_HIT_RADIUS: f32 = 10.0;

pub struct CurveTool {
    /// Id of the curve node being edited.
    pub node_id: String,
    /// The last-clicked control point — the Delete target.
    pub selected: Option<usize>,
    /// An in-flight drag, if a press grabbed (or just added) a handle.
    pub drag: Option<CurveDrag>,
}

#[derive(Clone, Copy)]
pub struct CurveDrag {
    pub point: usize,
    /// NDC depth captured at grab time; motion unprojects onto this plane.
    pub ndc_z: f32,
}

/// World → (screen x, screen y, ndc z) through the cached scene mvp.
pub fn project_point(mvp: &Mat4, view: (f32, f32, f32, f32), p: Vec3) -> Option<(f32, f32, f32)> {
    let (vx, vy, vw, vh) = view;
    let clip = *mvp * Vec4::new(p.x, p.y, p.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip / clip.w;
    Some((
        vx + (ndc.x * 0.5 + 0.5) * vw,
        vy + (0.5 - ndc.y * 0.5) * vh,
        ndc.z,
    ))
}

/// (screen x, screen y, ndc z) → world through the inverse of the cached mvp.
pub fn unproject_point(
    mvp: &Mat4,
    view: (f32, f32, f32, f32),
    sx: f32,
    sy: f32,
    ndc_z: f32,
) -> Option<Vec3> {
    let (vx, vy, vw, vh) = view;
    if vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    let inv = mvp.inverse();
    if !inv.is_finite() {
        return None;
    }
    let ndc_x = ((sx - vx) / vw - 0.5) * 2.0;
    let ndc_y = (0.5 - (sy - vy) / vh) * 2.0;
    let world = inv * Vec4::new(ndc_x, ndc_y, ndc_z, 1.0);
    if world.w.abs() < 1e-6 {
        return None;
    }
    let world = world / world.w;
    if !world.is_finite() {
        return None;
    }
    Some(Vec3::new(world.x, world.y, world.z))
}

pub fn find_node_by_id<'a>(root: &'a FsNode, id: &str) -> Option<&'a FsNode> {
    if root.id == id {
        return Some(root);
    }
    root.children.iter().find_map(|c| find_node_by_id(c, id))
}

pub fn find_node_by_id_mut<'a>(root: &'a mut FsNode, id: &str) -> Option<&'a mut FsNode> {
    if root.id == id {
        return Some(root);
    }
    root.children
        .iter_mut()
        .find_map(|c| find_node_by_id_mut(c, id))
}

impl State {
    /// Enter/exit the curve viewer state for the node in `slot` of the
    /// current directory. A different curve node retargets the tool.
    pub(crate) fn toggle_curve_tool(&mut self, slot: usize) {
        let Some(node) = self.current_dir().children.get(slot) else {
            return;
        };
        if !node.node_type.eq_ignore_ascii_case("curve") {
            return;
        }
        let id = node.id.clone();
        if self.curve_tool.as_ref().map(|t| t.node_id == id).unwrap_or(false) {
            self.curve_tool = None;
        } else {
            self.curve_tool = Some(CurveTool { node_id: id, selected: None, drag: None });
        }
    }

    /// The edited node's control points, or None if the node is gone (or is
    /// no longer a curve — a project reload can put anything at an old id).
    fn curve_node_points(&self, node_id: &str) -> Option<Vec<Vec3>> {
        let node = find_node_by_id(&self.fs_root, node_id)?;
        if !node.node_type.eq_ignore_ascii_case("curve") {
            return None;
        }
        Some(parse_curve_points(&node_param_str(node, "Points", "")))
    }

    /// Write the points back and run the same resync sequence as SetParam,
    /// so the scene, spreadsheet, and params pane all follow the edit.
    fn set_curve_points(&mut self, node_id: &str, pts: &[Vec3]) {
        let formatted = format_curve_points(pts);
        let Some(node) = find_node_by_id_mut(&mut self.fs_root, node_id) else {
            return;
        };
        if let Some(p) = node.params.iter_mut().find(|p| p.name == "Points") {
            p.default = formatted;
        } else {
            return;
        }
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
    }

    /// The active tool's handles as (point index, screen x, screen y, ndc z).
    /// Empty when the tool is off, the node is gone, or no scene mvp has been
    /// cached yet (a frame before the first scene staging).
    pub(crate) fn curve_tool_handles(&self) -> Vec<(usize, f32, f32, f32)> {
        let Some(tool) = &self.curve_tool else {
            return Vec::new();
        };
        let Some(mvp) = self.last_scene_mvp else {
            return Vec::new();
        };
        let Some(pts) = self.curve_node_points(&tool.node_id) else {
            return Vec::new();
        };
        pts.iter()
            .enumerate()
            .filter_map(|(i, p)| {
                project_point(&mvp, self.last_scene_view_rect, *p).map(|(sx, sy, z)| (i, sx, sy, z))
            })
            .collect()
    }

    /// The handle under the cursor, nearest first.
    fn curve_tool_handle_at_cursor(&self) -> Option<(usize, f32)> {
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        self.curve_tool_handles()
            .iter()
            .map(|(i, sx, sy, z)| (*i, ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt(), *z))
            .filter(|(_, d, _)| *d <= HANDLE_HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, z)| (i, z))
    }

    /// Left press in the viewport while the tool is active: grab the handle
    /// under the cursor, or append a new point at the cursor (at the last
    /// point's depth) and start dragging it. Returns false — letting the
    /// press fall through — only when the edited node no longer exists.
    pub(crate) fn curve_tool_press(&mut self) -> bool {
        let Some(tool) = &self.curve_tool else {
            return false;
        };
        let node_id = tool.node_id.clone();
        let Some(mut pts) = self.curve_node_points(&node_id) else {
            self.curve_tool = None;
            return false;
        };
        if let Some((idx, ndc_z)) = self.curve_tool_handle_at_cursor() {
            let tool = self.curve_tool.as_mut().expect("checked above");
            tool.selected = Some(idx);
            tool.drag = Some(CurveDrag { point: idx, ndc_z });
            return true;
        }
        // Empty space: add a point. Depth comes from the last control point
        // (or the world origin for an empty curve) so the new point lands in
        // the plane the user is already working in.
        let Some(mvp) = self.last_scene_mvp else {
            return true; // tool consumes viewport presses even pre-staging
        };
        let view = self.last_scene_view_rect;
        let ndc_z = pts
            .last()
            .and_then(|p| project_point(&mvp, view, *p))
            .map(|(_, _, z)| z)
            .or_else(|| project_point(&mvp, view, Vec3::ZERO).map(|(_, _, z)| z));
        let Some(ndc_z) = ndc_z else {
            return true;
        };
        let Some(world) = unproject_point(&mvp, view, self.cursor_x, self.cursor_y, ndc_z) else {
            return true;
        };
        pts.push(world);
        let idx = pts.len() - 1;
        self.set_curve_points(&node_id, &pts);
        if let Some(tool) = self.curve_tool.as_mut() {
            tool.selected = Some(idx);
            tool.drag = Some(CurveDrag { point: idx, ndc_z });
        }
        true
    }

    /// Pointer motion during a grab: the point tracks the cursor on the
    /// camera-facing plane at its grab depth.
    pub(crate) fn curve_tool_drag_motion(&mut self) -> bool {
        let Some(drag) = self.curve_tool.as_ref().and_then(|t| t.drag) else {
            return false;
        };
        let Some(mvp) = self.last_scene_mvp else {
            return false;
        };
        let node_id = self.curve_tool.as_ref().expect("drag implies tool").node_id.clone();
        let Some(mut pts) = self.curve_node_points(&node_id) else {
            self.curve_tool = None;
            return false;
        };
        if drag.point >= pts.len() {
            return false;
        }
        let Some(world) =
            unproject_point(&mvp, self.last_scene_view_rect, self.cursor_x, self.cursor_y, drag.ndc_z)
        else {
            return false;
        };
        pts[drag.point] = world;
        self.set_curve_points(&node_id, &pts);
        true
    }

    /// Button release: end any in-flight grab.
    pub(crate) fn curve_tool_release(&mut self) -> bool {
        match self.curve_tool.as_mut() {
            Some(tool) if tool.drag.is_some() => {
                tool.drag = None;
                true
            }
            _ => false,
        }
    }

    /// Right press while the tool is active: delete the handle under the
    /// cursor. Consumes only on a hit — otherwise the press falls through to
    /// the viewport context menu.
    pub(crate) fn curve_tool_delete_at_cursor(&mut self) -> bool {
        let Some((idx, _)) = self.curve_tool_handle_at_cursor() else {
            return false;
        };
        self.curve_tool_delete_point(idx)
    }

    /// Delete/Backspace: remove the selected control point, if any.
    pub(crate) fn curve_tool_delete_selected(&mut self) -> bool {
        let Some(idx) = self.curve_tool.as_ref().and_then(|t| t.selected) else {
            return false;
        };
        self.curve_tool_delete_point(idx)
    }

    fn curve_tool_delete_point(&mut self, idx: usize) -> bool {
        let Some(tool) = &self.curve_tool else {
            return false;
        };
        let node_id = tool.node_id.clone();
        let Some(mut pts) = self.curve_node_points(&node_id) else {
            self.curve_tool = None;
            return false;
        };
        if idx >= pts.len() {
            return false;
        }
        pts.remove(idx);
        self.set_curve_points(&node_id, &pts);
        if let Some(tool) = self.curve_tool.as_mut() {
            tool.drag = None;
            // Keep a neighbor selected so repeated Delete walks the curve.
            tool.selected = if pts.is_empty() {
                None
            } else {
                Some(idx.min(pts.len() - 1))
            };
        }
        true
    }
}
