//! The viewer-state framework: interactive viewport tools, generalized out of
//! the curve tool.
//!
//! A viewer state is a mode the viewport is in, bound to one node, in which
//! the pointer edits that node directly instead of orbiting the camera. The
//! curve tool was the first, and everything in it except "what a handle IS"
//! turned out to be the same for any such tool:
//!
//! - **projection** — world positions to screen through the raster scene's
//!   cached mvp, the same path the meta Point Numbers overlay rides;
//! - **hit-testing** — the nearest handle within a radius of the cursor;
//! - **the drag model** — capture the grabbed handle's NDC depth, then
//!   unproject the cursor onto that plane, so orbiting between edits never
//!   makes a drag jump;
//! - **per-gesture undo** — a whole drag is one step, recorded on the first
//!   motion after a grab so a click that never moves records nothing;
//! - **binding by node ID, not slot** — renames and graph edits do not detach
//!   the tool, and a node that disappears makes every handler resolve nothing
//!   so the state drops out lazily;
//! - **write-back** — the same resync sequence `McpAction::SetParam` runs, so
//!   the params pane, the spreadsheet and the scene all follow an edit live.
//!
//! What differs per tool is [`HandleSource`]: which node types it accepts,
//! where the handles are, how to write them back, and whether the pointer may
//! add and remove them. Two implementations ship, deliberately different in
//! shape — a curve's open-ended list of control points, and a soft transform's
//! fixed pair where one handle's position is DERIVED from two parameters. An
//! abstraction with a single implementation has not been shown to be one.
//!
//! The two things the framework adds over what the curve tool had are snapping
//! and a HUD, both of which every tool wants and neither of which a tool
//! should implement itself.

use crate::app::{FsNode, State};
use cce_ui::history::History;
use glam::{Mat4, Vec3, Vec4};

/// How close (logical px) a press must land to a projected handle to grab it.
pub const HANDLE_HIT_RADIUS: f32 = 10.0;

/// What a viewer state edits.
///
/// Handles are WORLD positions, always. A source whose parameters are not
/// world positions — a soft transform's translation is an offset — converts in
/// [`read`](Self::read) and [`write`](Self::write), so the framework never has
/// to know the difference and the drag maths stays one implementation.
/// What a source may need to know beyond its own node.
///
/// A curve's points are world positions and need nothing. A shape on an
/// image is written in the image's unit from the image's corner, so where it
/// stands in the scene depends on a node further up its chain and on what a
/// world unit is — neither of which `write` can look up, holding the node
/// mutably as it does. The framework gathers both before it calls.
#[derive(Clone, Copy, Debug, Default)]
pub struct HandleCtx {
    /// The page the node draws on, when it draws on one.
    pub page: Option<crate::page::PageFrame>,
    /// One world unit in millimetres.
    pub world_unit_mm: f32,
}

pub trait HandleSource {
    /// Shown in the HUD, so it says what mode the viewport is in.
    fn name(&self) -> &'static str;

    /// Whether this source can edit a node of that type. Checked on every
    /// resolution, not just on entry: a project reload can put anything at an
    /// old id, and the tool must drop out rather than write nonsense.
    fn accepts(&self, node_type: &str) -> bool;

    /// The node's handles, in world space.
    fn read(&self, node: &FsNode, ctx: &HandleCtx) -> Vec<Vec3>;

    /// Write handles back into the node's parameters. The framework runs the
    /// resync afterwards.
    fn write(&self, node: &mut FsNode, handles: &[Vec3], ctx: &HandleCtx);

    /// Handle `moved` was dragged to `to`: what the whole set is now.
    ///
    /// Moving the one handle is the default, and is all a source of
    /// independent handles wants. A source whose handles hang off one
    /// another carries them here — a shape's corner goes with its centre —
    /// so that `write` is still handed a whole set that means one thing,
    /// whether it came from a drag or from an undo snapshot.
    fn drag(&self, handles: &mut Vec<Vec3>, moved: usize, to: Vec3, _ctx: &HandleCtx) {
        handles[moved] = to;
    }

    /// The plane the handles live in, as a point on it and its normal, for
    /// a source that has one. A drag then follows the cursor's ray to that
    /// plane, where without one it follows it to the camera-facing plane at
    /// the depth the handle was grabbed — which leaves a flat thing's plane
    /// as soon as the view is not square to it.
    fn plane(&self, _ctx: &HandleCtx) -> Option<(Vec3, Vec3)> {
        None
    }

    /// A closed outline to draw with the handles, in world space: what is
    /// being edited, where the handles alone do not show it.
    fn outline(&self, _node: &FsNode, _ctx: &HandleCtx) -> Vec<Vec3> {
        Vec::new()
    }

    /// Whether the handles are joined in order by a faint line — a curve's
    /// control cage. False for handles that are not a sequence.
    fn cage(&self) -> bool {
        true
    }

    /// Whether a press on empty space appends a handle and a right press
    /// deletes one. False for a source with a fixed set — a soft transform has
    /// exactly a centre and a translation, and a third handle would mean
    /// nothing.
    fn extensible(&self) -> bool;

    /// The key hints for the HUD, without the ones the framework owns
    /// (snapping, Escape) — those are appended.
    fn hints(&self) -> &'static str;

    /// What to write beside handle `i`. Indices by default, which is right
    /// for an ordered list; a source whose handles mean different things names
    /// them instead, because "1" and "2" on a centre and a tip is a worse
    /// label than none.
    fn handle_label(&self, i: usize) -> String {
        (i + 1).to_string()
    }
}

/// An in-flight drag.
#[derive(Clone, Copy)]
pub struct Drag {
    pub handle: usize,
    /// NDC depth captured at grab time; motion unprojects onto this plane.
    pub ndc_z: f32,
}

/// The active viewer state.
pub struct ViewerTool {
    /// The edited node's id — not its slot, so renames and graph edits do not
    /// detach the tool.
    pub node_id: String,
    pub source: Box<dyn HandleSource>,
    /// The last-clicked handle — the Delete target.
    pub selected: Option<usize>,
    pub drag: Option<Drag>,
    /// Handle snapshots, one per gesture.
    pub history: History<Vec<Vec3>>,
    /// World-space increment a dragged handle rounds to, or `None` for free
    /// movement. Lives on the tool rather than in settings because it is a
    /// property of the editing session, and it survives retargeting so turning
    /// it on does not have to be repeated per node.
    pub snap: Option<f32>,
}

/// The increment snapping rounds to when it is switched on.
///
/// A tenth of a world unit: fine enough to place a point deliberately, coarse
/// enough that two snapped points actually coincide. The world unit is a
/// DECLARATION here (see the Guides node), so this is a tenth of whatever the
/// project says a unit is rather than a tenth of a millimetre.
pub const SNAP_INCREMENT: f32 = 0.1;

impl ViewerTool {
    pub fn new(node_id: String, source: Box<dyn HandleSource>) -> Self {
        ViewerTool { node_id, source, selected: None, drag: None, history: History::new(), snap: None }
    }

    /// The HUD line: what mode this is, what the keys do, and whether snapping
    /// is on. The snap state is on the HUD because it silently changes what a
    /// drag does, and a mode you cannot see is a mode you forget you are in.
    pub fn hud(&self) -> String {
        format!(
            "{} — {}  ·  Snap {}  ·  Esc exits",
            self.source.name(),
            self.source.hints(),
            if self.snap.is_some() { "on" } else { "off" },
        )
    }
}

/// Round a world position to `increment` on every axis.
fn snapped(p: Vec3, increment: Option<f32>) -> Vec3 {
    match increment {
        Some(i) if i > 0.0 => Vec3::new(
            (p.x / i).round() * i,
            (p.y / i).round() * i,
            (p.z / i).round() * i,
        ),
        _ => p,
    }
}

/// World → (screen x, screen y, ndc z) through the cached scene mvp.
pub fn project_point(mvp: &Mat4, view: (f32, f32, f32, f32), p: Vec3) -> Option<(f32, f32, f32)> {
    let (vx, vy, vw, vh) = view;
    let clip = *mvp * Vec4::new(p.x, p.y, p.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip / clip.w;
    Some((vx + (ndc.x * 0.5 + 0.5) * vw, vy + (0.5 - ndc.y * 0.5) * vh, ndc.z))
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
    root.children.iter_mut().find_map(|c| find_node_by_id_mut(c, id))
}

/// The viewer state a node type can enter, if any.
///
/// One place that maps node types to tools, so the node context menu, the
/// command and any future entry point agree about what is editable.
pub fn source_for(node_type: &str) -> Option<Box<dyn HandleSource>> {
    let sources: [Box<dyn HandleSource>; 4] = [
        Box::new(crate::curve_tool::CurveHandles),
        Box::new(crate::soft_transform_tool::SoftTransformHandles),
        Box::new(crate::image_handles::ShapeHandles),
        Box::new(crate::image_handles::TextHandles),
    ];
    sources.into_iter().find(|s| s.accepts(node_type))
}

impl State {
    /// Enter/exit the viewer state for the node in `slot` of the current
    /// directory. A different editable node retargets the tool.
    pub(crate) fn toggle_viewer_state(&mut self, slot: usize) {
        let Some(node) = self.current_dir().children.get(slot) else { return };
        let Some(source) = source_for(&node.node_type) else { return };
        let id = node.id.clone();
        if self.viewer_tool.as_ref().map(|t| t.node_id == id).unwrap_or(false) {
            self.viewer_tool = None;
        } else {
            self.viewer_tool = Some(ViewerTool::new(id, source));
        }
    }

    /// Turn snapping on or off for the active state. Returns false when no
    /// state is active, so the command falls through to mean nothing rather
    /// than reporting success.
    pub(crate) fn toggle_viewer_snap(&mut self) -> bool {
        let Some(tool) = self.viewer_tool.as_mut() else { return false };
        tool.snap = match tool.snap {
            Some(_) => None,
            None => Some(SNAP_INCREMENT),
        };
        let on = tool.snap.is_some();
        self.update_status_text(if on { "Snapping on" } else { "Snapping off" });
        true
    }

    /// What the source of the node's handles may need to know: gathered
    /// here, once, ahead of every read and write.
    fn viewer_handle_ctx(&self, node: &FsNode) -> HandleCtx {
        let page = crate::page::is_page_node(&node.node_type)
            .then(|| crate::page::resolve_frame(&self.fs_root, node))
            .flatten();
        HandleCtx { page, world_unit_mm: self.world_unit_mm() }
    }

    /// The edited node's handles, or None if the node is gone or is no longer
    /// a type this source accepts.
    fn viewer_handles_of(&self, node_id: &str) -> Option<Vec<Vec3>> {
        let tool = self.viewer_tool.as_ref()?;
        let node = find_node_by_id(&self.fs_root, node_id)?;
        if !tool.source.accepts(&node.node_type) {
            return None;
        }
        Some(tool.source.read(node, &self.viewer_handle_ctx(node)))
    }

    /// The active tool's outline, projected: screen points of a closed
    /// loop, empty for a source that draws none.
    pub(crate) fn viewer_tool_outline(&self) -> Vec<(f32, f32)> {
        let Some(tool) = &self.viewer_tool else { return Vec::new() };
        let Some(mvp) = self.last_scene_mvp else { return Vec::new() };
        let Some(node) = find_node_by_id(&self.fs_root, &tool.node_id) else { return Vec::new() };
        if !tool.source.accepts(&node.node_type) {
            return Vec::new();
        }
        let loop_ = tool.source.outline(node, &self.viewer_handle_ctx(node));
        let projected: Vec<(f32, f32)> = loop_
            .iter()
            .filter_map(|p| project_point(&mvp, self.last_scene_view_rect, *p))
            .map(|(x, y, _)| (x, y))
            .collect();
        // A corner behind the camera has no place on screen, and a loop
        // missing one is another shape.
        if projected.len() == loop_.len() { projected } else { Vec::new() }
    }

    /// Where a drag puts the grabbed handle: on the source's plane under
    /// the cursor when it has one, else on the camera-facing plane at the
    /// depth the handle was grabbed.
    fn viewer_drag_target(&self, node_id: &str, ndc_z: f32) -> Option<Vec3> {
        let mvp = self.last_scene_mvp?;
        let view = self.last_scene_view_rect;
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        let plane = self.viewer_tool.as_ref().and_then(|tool| {
            let node = find_node_by_id(&self.fs_root, node_id)?;
            tool.source.plane(&self.viewer_handle_ctx(node))
        });
        if let Some((point, normal)) = plane {
            let near = unproject_point(&mvp, view, cx, cy, 0.0)?;
            let far = unproject_point(&mvp, view, cx, cy, 1.0)?;
            let ray = far - near;
            let along = ray.dot(normal);
            // A plane seen edge-on is met nowhere the cursor can name.
            if along.abs() > 1e-6 * ray.length().max(1e-6) {
                let t = (point - near).dot(normal) / along;
                return Some(near + ray * t);
            }
        }
        unproject_point(&mvp, view, cx, cy, ndc_z)
    }

    /// Write handles back and run the same resync sequence as SetParam.
    fn set_viewer_handles(&mut self, node_id: &str, handles: &[Vec3]) {
        let ctx = match find_node_by_id(&self.fs_root, node_id) {
            Some(node) => self.viewer_handle_ctx(node),
            None => return,
        };
        let Some(tool) = self.viewer_tool.take() else { return };
        if let Some(node) = find_node_by_id_mut(&mut self.fs_root, node_id) {
            tool.source.write(node, handles, &ctx);
        }
        self.viewer_tool = Some(tool);
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
    }

    /// The active tool's handles as (index, screen x, screen y, ndc z).
    /// Empty when no tool is active, the node is gone, or no scene mvp has
    /// been cached yet (a frame before the first scene staging).
    pub(crate) fn viewer_tool_handles(&self) -> Vec<(usize, f32, f32, f32)> {
        let Some(tool) = &self.viewer_tool else { return Vec::new() };
        let Some(mvp) = self.last_scene_mvp else { return Vec::new() };
        let Some(pts) = self.viewer_handles_of(&tool.node_id) else { return Vec::new() };
        pts.iter()
            .enumerate()
            .filter_map(|(i, p)| {
                project_point(&mvp, self.last_scene_view_rect, *p).map(|(sx, sy, z)| (i, sx, sy, z))
            })
            .collect()
    }

    /// The handle under the cursor, nearest first.
    fn viewer_handle_at_cursor(&self) -> Option<(usize, f32)> {
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        self.viewer_tool_handles()
            .iter()
            .map(|(i, sx, sy, z)| (*i, ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt(), *z))
            .filter(|(_, d, _)| *d <= HANDLE_HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, z)| (i, z))
    }

    /// Left press in the viewport while a state is active: grab the handle
    /// under the cursor, or — for an extensible source — append a new handle
    /// there and start dragging it. Returns false, letting the press fall
    /// through, only when the edited node no longer exists.
    pub(crate) fn viewer_tool_press(&mut self) -> bool {
        let Some(tool) = &self.viewer_tool else { return false };
        let node_id = tool.node_id.clone();
        let Some(mut pts) = self.viewer_handles_of(&node_id) else {
            self.viewer_tool = None;
            return false;
        };
        if let Some((idx, ndc_z)) = self.viewer_handle_at_cursor() {
            let tool = self.viewer_tool.as_mut().expect("checked above");
            tool.selected = Some(idx);
            tool.history.begin_gesture(pts);
            tool.drag = Some(Drag { handle: idx, ndc_z });
            return true;
        }
        if !self.viewer_tool.as_ref().is_some_and(|t| t.source.extensible()) {
            // A fixed source consumes the press anyway: the state owns the
            // viewport while it is active, and falling through would open the
            // context menu on every miss.
            return true;
        }
        // Empty space: add a handle. Depth comes from the last one (or the
        // world origin) so the new handle lands in the plane already in use.
        let Some(mvp) = self.last_scene_mvp else { return true };
        let view = self.last_scene_view_rect;
        let ndc_z = pts
            .last()
            .and_then(|p| project_point(&mvp, view, *p))
            .map(|(_, _, z)| z)
            .or_else(|| project_point(&mvp, view, Vec3::ZERO).map(|(_, _, z)| z));
        let Some(ndc_z) = ndc_z else { return true };
        let Some(world) = unproject_point(&mvp, view, self.cursor_x, self.cursor_y, ndc_z) else {
            return true;
        };
        let snap = self.viewer_tool.as_ref().and_then(|t| t.snap);
        let before = pts.clone();
        pts.push(snapped(world, snap));
        let idx = pts.len() - 1;
        self.set_viewer_handles(&node_id, &pts);
        if let Some(tool) = self.viewer_tool.as_mut() {
            // The add is the recorded step; the drag that follows is part of
            // the same gesture, so no gesture is opened for it.
            tool.history.record(before);
            tool.selected = Some(idx);
            tool.drag = Some(Drag { handle: idx, ndc_z });
        }
        true
    }

    /// Pointer motion during a grab: the handle tracks the cursor on the
    /// camera-facing plane at its grab depth.
    pub(crate) fn viewer_tool_drag_motion(&mut self) -> bool {
        let Some(drag) = self.viewer_tool.as_ref().and_then(|t| t.drag) else { return false };
        if self.last_scene_mvp.is_none() {
            return false;
        }
        let node_id = self.viewer_tool.as_ref().expect("drag implies tool").node_id.clone();
        let Some(mut pts) = self.viewer_handles_of(&node_id) else {
            self.viewer_tool = None;
            return false;
        };
        if drag.handle >= pts.len() {
            return false;
        }
        let Some(world) = self.viewer_drag_target(&node_id, drag.ndc_z) else {
            return false;
        };
        let snap = self.viewer_tool.as_ref().and_then(|t| t.snap);
        let ctx = match find_node_by_id(&self.fs_root, &node_id) {
            Some(node) => self.viewer_handle_ctx(node),
            None => return false,
        };
        if let Some(tool) = self.viewer_tool.as_ref() {
            tool.source.drag(&mut pts, drag.handle, snapped(world, snap), &ctx);
        }
        if let Some(tool) = self.viewer_tool.as_mut() {
            tool.history.commit_gesture();
        }
        self.set_viewer_handles(&node_id, &pts);
        true
    }

    /// Button release: end any in-flight grab.
    pub(crate) fn viewer_tool_release(&mut self) -> bool {
        match self.viewer_tool.as_mut() {
            Some(tool) if tool.drag.is_some() => {
                tool.drag = None;
                tool.history.cancel_gesture();
                true
            }
            _ => false,
        }
    }

    /// Right press: delete the handle under the cursor. Consumes only on a hit
    /// on an extensible source — otherwise the press falls through to the
    /// viewport context menu.
    pub(crate) fn viewer_tool_delete_at_cursor(&mut self) -> bool {
        if !self.viewer_tool.as_ref().is_some_and(|t| t.source.extensible()) {
            return false;
        }
        let Some((idx, _)) = self.viewer_handle_at_cursor() else { return false };
        self.viewer_tool_delete_handle(idx)
    }

    /// Delete/Backspace: remove the selected handle, if any.
    pub(crate) fn viewer_tool_delete_selected(&mut self) -> bool {
        if !self.viewer_tool.as_ref().is_some_and(|t| t.source.extensible()) {
            return false;
        }
        let Some(idx) = self.viewer_tool.as_ref().and_then(|t| t.selected) else { return false };
        self.viewer_tool_delete_handle(idx)
    }

    fn viewer_tool_delete_handle(&mut self, idx: usize) -> bool {
        let Some(tool) = &self.viewer_tool else { return false };
        let node_id = tool.node_id.clone();
        let Some(mut pts) = self.viewer_handles_of(&node_id) else {
            self.viewer_tool = None;
            return false;
        };
        if idx >= pts.len() {
            return false;
        }
        let before = pts.clone();
        pts.remove(idx);
        self.set_viewer_handles(&node_id, &pts);
        if let Some(tool) = self.viewer_tool.as_mut() {
            tool.history.record(before);
            tool.drag = None;
            // Keep a neighbour selected so repeated Delete walks the handles.
            tool.selected = if pts.is_empty() { None } else { Some(idx.min(pts.len() - 1)) };
        }
        true
    }

    /// Undo: return the handles to how they were before the last recorded
    /// gesture. Consumes only when a state is active and has history.
    pub(crate) fn viewer_tool_undo(&mut self) -> bool {
        self.viewer_tool_step(true)
    }

    /// Redo: reapply the last undone gesture.
    pub(crate) fn viewer_tool_redo(&mut self) -> bool {
        self.viewer_tool_step(false)
    }

    fn viewer_tool_step(&mut self, undo: bool) -> bool {
        let Some(tool) = &self.viewer_tool else { return false };
        let node_id = tool.node_id.clone();
        let Some(current) = self.viewer_handles_of(&node_id) else {
            self.viewer_tool = None;
            return false;
        };
        let tool = self.viewer_tool.as_mut().expect("checked above");
        let stepped = if undo { tool.history.undo(current) } else { tool.history.redo(current) };
        let Some(target) = stepped else { return false };
        // A step mid-drag abandons the drag: the grabbed index may not exist
        // in the restored list, and the pointer no longer means anything to it.
        tool.drag = None;
        tool.selected = match tool.selected {
            Some(i) if !target.is_empty() => Some(i.min(target.len() - 1)),
            _ => None,
        };
        self.set_viewer_handles(&node_id, &target);
        true
    }
}
