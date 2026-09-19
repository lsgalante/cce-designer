//! The soft-transform viewer state: place the falloff centre and the offset by
//! dragging, instead of typing six numbers.
//!
//! The second [`HandleSource`], and deliberately a different shape from the
//! curve's — an abstraction with one implementation has not been shown to be
//! one. Where a curve is an open-ended list of world positions stored as world
//! positions, a soft transform has exactly TWO handles and only one of them is
//! a position:
//!
//! - **Centre** — where the falloff is centred, a world position, stored as
//!   one.
//! - **Tip** — drawn at `Centre + Translation`, because a translation is not a
//!   place: it is how far things move.
//!
//! So the pair reads as a vector with a base and a tip, and dragging EITHER
//! end changes the offset between them — moving the centre keeps the tip where
//! it is and shortens or lengthens the translation to match. That is the
//! behaviour a two-handled gizmo has everywhere, and it is also the only one
//! this trait can express honestly: `write` is handed a full set of handles
//! with no word about which moved, and it has to mean the same thing when the
//! set came from an undo snapshot as when it came from a drag. A rule like
//! "keep the translation when the centre moves" would be right for the drag
//! and would quietly discard half of every restored snapshot.
//!
//! The conversion between stored parameters and world handles is exactly what
//! [`HandleSource`] exists to contain. The framework's drag maths only ever
//! sees two world positions, and needs no idea that one of them is derived —
//! which is the property that makes it a framework rather than the curve tool
//! with the names changed.
//!
//! Handles are fixed, so the source is not extensible: a press on empty space
//! grabs nothing and a third handle would mean nothing.

use crate::app::FsNode;
use crate::geometry::node_param_str;
use crate::viewer_state::HandleSource;
use glam::Vec3;

pub struct SoftTransformHandles;

/// Read a `x:y:z` (or whitespace/comma separated) triple parameter.
///
/// Soft Transform stores Translation and Center as `text`, not `float3`, so
/// the separator it was saved with depends on which pane wrote it last —
/// accept any of them rather than silently reading a zero.
fn triple(node: &FsNode, name: &str) -> Vec3 {
    let raw = node_param_str(node, name, "");
    let n: Vec<f32> = raw
        .split(|c: char| c == ':' || c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f32>().ok())
        .collect();
    if n.len() == 3 && n.iter().all(|v| v.is_finite()) {
        Vec3::new(n[0], n[1], n[2])
    } else {
        Vec3::ZERO
    }
}

fn set_triple(node: &mut FsNode, name: &str, v: Vec3) {
    // Written back in the same `x:y:z` form the templates ship, so a value the
    // tool wrote and a value the pane wrote are indistinguishable.
    let formatted = format!("{:.2}:{:.2}:{:.2}", v.x, v.y, v.z);
    if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
        p.default = formatted;
    }
}

impl HandleSource for SoftTransformHandles {
    fn name(&self) -> &'static str {
        "Soft Transform"
    }

    fn accepts(&self, node_type: &str) -> bool {
        node_type.eq_ignore_ascii_case("soft_transform")
    }

    fn read(&self, node: &FsNode) -> Vec<Vec3> {
        let centre = triple(node, "Center");
        vec![centre, centre + triple(node, "Translation")]
    }

    fn write(&self, node: &mut FsNode, handles: &[Vec3]) {
        let [centre, offset] = handles else { return };
        set_triple(node, "Center", *centre);
        set_triple(node, "Translation", *offset - *centre);
    }

    fn extensible(&self) -> bool {
        false
    }

    fn hints(&self) -> &'static str {
        "drag the base to place the falloff, the tip to set the offset"
    }

    fn handle_label(&self, i: usize) -> String {
        if i == 0 { "centre".into() } else { "tip".into() }
    }
}
