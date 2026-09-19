//! The curve viewer state: the `curve` node's control points as draggable
//! handles, entered from the node's context menu ("Edit Handles").
//!
//! This is now one [`HandleSource`] on the framework in
//! [`crate::viewer_state`], which owns everything that used to live here:
//! projection, hit-testing, the drag model, per-gesture undo, snapping, the
//! HUD, and write-back through the SetParam resync sequence. What is left is
//! the part that is actually about curves — an open-ended list of world
//! positions in the "Points" parameter, which the pointer may extend and trim.
//!
//! The behaviour that survives the move, because it is the framework's now:
//!
//! - **left press on a handle** grabs it; dragging moves the point on the
//!   camera-facing plane at its own depth, so orbiting between edits never
//!   makes a drag jump;
//! - **left press on empty space** appends a point there, at the depth of the
//!   last one, and immediately drags it;
//! - **right press on a handle** deletes it; Delete/Backspace deletes the
//!   selected one;
//! - **undo / redo** step one gesture at a time — a whole drag is one step,
//!   an add-and-drag is one step, a delete is one step;
//! - **Escape** exits.

use crate::app::FsNode;
use crate::geometry::{format_curve_points, node_param_str, parse_curve_points};
use crate::viewer_state::HandleSource;
use glam::Vec3;

/// Re-exported for the call sites that predate the framework. The projection
/// helpers are the framework's; this module is only the curve's handles.
pub use crate::viewer_state::{
    find_node_by_id, find_node_by_id_mut, project_point, unproject_point, HANDLE_HIT_RADIUS,
};

pub struct CurveHandles;

impl HandleSource for CurveHandles {
    fn name(&self) -> &'static str {
        "Curve Points"
    }

    fn accepts(&self, node_type: &str) -> bool {
        node_type.eq_ignore_ascii_case("curve")
    }

    fn read(&self, node: &FsNode) -> Vec<Vec3> {
        parse_curve_points(&node_param_str(node, "Points", ""))
    }

    fn write(&self, node: &mut FsNode, handles: &[Vec3]) {
        let formatted = format_curve_points(handles);
        if let Some(p) = node.params.iter_mut().find(|p| p.name == "Points") {
            p.default = formatted;
        }
    }

    fn extensible(&self) -> bool {
        true
    }

    fn hints(&self) -> &'static str {
        "drag to move, click to add, right-click or Del to remove"
    }
}
