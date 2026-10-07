//! Where a node may stand: the OBJECT level and the GEOMETRY context.
//!
//! The root is the object level, Houdini's `/obj`. Geometry is not built
//! there: it is built inside a `geometry` node, a container that stands at
//! the root and is what the root draws, and everything that makes or
//! changes geometry is placed inside one (at any depth — a subnet, a simnet
//! or a repeat inside a geometry node is in its context too). Since
//! 2026-10-02; until then every node could stand anywhere and the root was
//! one big geometry level.
//!
//! Three placements, by node type ([`placement`]):
//!
//! - **Object** — the root only: the `geometry` container itself, cameras,
//!   which are seen from every level (`State::camera_level`), and the
//!   `environment` node, the scene's light (`crate::environment`).
//! - **Geometry** — inside a geometry node only: every operator, subnets,
//!   simnets, repeats and the subnet templates (the Embryo, the Remesh).
//! - **Any** — the page nodes, which are a 2D context of their own and stay
//!   where they always could, and `export`, which writes a page or a mesh.
//!
//! The rule is held where a node ARRIVES at a level — Add Node, paste, MCP's
//! `add_node` — and by the load ([`wrap_root_geometry`], format 5). It is not
//! held by the evaluator: a hand-built tree with a sphere at the root still
//! draws, which is what keeps the suite's fixtures meaning what they meant.

use crate::app::{FsNode, ParamKind, Project};

/// The container's node type.
pub const GEOMETRY: &str = "geometry";

/// What a level is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Object,
    Geometry,
}

/// Where a node of some type may stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Object,
    Geometry,
    Any,
}

pub fn is_geometry_container(node_type: &str) -> bool {
    node_type.eq_ignore_ascii_case(GEOMETRY)
}

/// Where a node of `node_type` may stand. Everything not named is an
/// operator, so a new node type is a geometry node without a line here.
pub fn placement(node_type: &str) -> Placement {
    if is_geometry_container(node_type)
        || node_type.eq_ignore_ascii_case("camera")
        || node_type.eq_ignore_ascii_case(crate::environment::ENVIRONMENT)
    {
        Placement::Object
    } else if crate::page::is_page_node(node_type) || node_type.eq_ignore_ascii_case("export") {
        Placement::Any
    } else {
        Placement::Geometry
    }
}

/// The context of the level a network editor's path addresses. The root is
/// the object level and every level under it is inside a geometry node:
/// the root's only enterable nodes are geometry containers, since a subnet
/// is a geometry node. (A hand-built tree with a subnet at the root is in
/// the geometry context inside it too, which is what it was built to be.)
pub fn context_at(path: &[usize]) -> Context {
    if path.is_empty() { Context::Object } else { Context::Geometry }
}

pub fn fits(placement: Placement, context: Context) -> bool {
    match placement {
        Placement::Any => true,
        Placement::Object => context == Context::Object,
        Placement::Geometry => context == Context::Geometry,
    }
}

/// Why `label` (a template's label or a node's name) cannot go into a level
/// of `context` — the status line's text — or None when it can.
pub fn refusal(label: &str, node_type: &str, context: Context) -> Option<String> {
    match placement(node_type) {
        p if fits(p, context) => None,
        Placement::Geometry => Some(format!("{label} goes inside a Geometry node: add one here and dive in")),
        _ => Some(format!("{label} goes at the root")),
    }
}

/// The root's first geometry container, made when there is none: where a
/// geometry node that has arrived at the root is re-homed. Its slot.
pub fn geometry_home(root: &mut FsNode) -> usize {
    if let Some(i) = root.children.iter().position(|c| is_geometry_container(&c.node_type)) {
        return i;
    }
    let name = unused_name(root, "geometry");
    let position = free_cell(root, (0.0, 0.0));
    root.children.push(container(name, position));
    root.children.len() - 1
}

fn container(name: String, position: (f32, f32)) -> FsNode {
    FsNode {
        id: crate::app::generate_node_id(),
        name,
        node_type: GEOMETRY.to_string(),
        children: Vec::new(),
        params: Vec::new(),
        geometry_visible: true,
        bypassed: false,
        position,
        inputs: 0,
        outputs: 0,
    }
}

fn unused_name(level: &FsNode, base: &str) -> String {
    (1..)
        .map(|i| format!("{base}{i}"))
        .find(|n| !level.children.iter().any(|c| &c.name == n))
        .unwrap()
}

/// The first cell at or right of `at` no child of `level` stands on.
pub fn free_cell(level: &FsNode, at: (f32, f32)) -> (f32, f32) {
    let mut cell = at;
    while level.children.iter().any(|c| c.position == cell) {
        cell.0 += 1.0;
    }
    cell
}

/// Whether a root child stays at the root when an older save is carried
/// into the object level. The retired settings nodes stay so that
/// `migrate_meta_settings_node`, which runs after, finds them where they
/// were; an export stays only when it reads a page standing at the root,
/// since what it writes is then a page.
fn stays_at_root(root: &FsNode, node: &FsNode) -> bool {
    if matches!(node.node_type.as_str(), "session" | "meta" | "utility") {
        return true;
    }
    if node.node_type.eq_ignore_ascii_case("export") {
        let input = crate::geometry::node_param_node(node, "input");
        return input.is_some_and(|name| {
            root.children.iter().any(|c| c.name == name && crate::page::is_page_node(&c.node_type))
        });
    }
    placement(&node.node_type) != Placement::Geometry
}

/// Format 4 → 5: the root becomes the object level. Every root child that
/// is a geometry node goes into one new `geometry` container at the root,
/// in its order, with its position, wires and flags — so it reads what it
/// read, since a wire looks among its siblings first and they came along.
/// Cameras, pages and the retired settings nodes stay.
///
/// What NAMES a moved node from somewhere else is re-pointed: a channel
/// path, in an expression or a wrangle's Code, that crosses between the
/// container and the root (relative) or reaches into it from the top
/// (absolute) is resolved where it stood and written again from where it
/// stands. And the view follows: an editor looking at the root looks into
/// the container, at the node it had selected, and a path into a moved
/// subnet goes through the container.
pub fn wrap_root_geometry(project: &mut Project) {
    let old = project.root.clone();
    let moving: Vec<bool> = old.children.iter().map(|c| !stays_at_root(&old, c)).collect();
    if !moving.contains(&true) {
        return;
    }

    // Every channel path anywhere, resolved in the tree as it stands.
    let refs = channel_refs(&old);

    let mut kept = Vec::new();
    let mut moved = Vec::new();
    // Old slot → where it went: (in the container?, new slot).
    let mut map = Vec::new();
    for (child, go) in old.children.iter().cloned().zip(&moving) {
        if *go {
            map.push((true, moved.len()));
            moved.push(child);
        } else {
            map.push((false, kept.len()));
            kept.push(child);
        }
    }
    let corner = moved.iter().fold((f32::MAX, f32::MAX), |(x, y), n| (x.min(n.position.0), y.min(n.position.1)));
    let mut root = old.clone();
    root.children = kept;
    let name = unused_name(&root, GEOMETRY);
    let position = free_cell(&root, corner);
    let mut geo = container(name, position);
    geo.children = moved;
    let geo_slot = root.children.len();
    root.children.push(geo);

    repoint_channel_refs(&mut root, &refs);
    project.root = root;

    let remap = |path: &[usize]| -> Vec<usize> {
        match path.split_first() {
            None => vec![geo_slot],
            Some((&first, rest)) => match map.get(first) {
                Some(&(true, slot)) => [&[geo_slot, slot][..], rest].concat(),
                Some(&(false, slot)) => [&[slot][..], rest].concat(),
                None => Vec::new(),
            },
        }
    };
    let view = &mut project.view_state;
    if view.current_path.is_empty() {
        match view.selected_node.and_then(|s| map.get(s).copied()) {
            // A camera or a page selected at the root: the editor stays.
            Some((false, slot)) => view.selected_node = Some(slot),
            Some((true, slot)) => {
                view.current_path = vec![geo_slot];
                view.selected_node = Some(slot);
            }
            None => {
                view.current_path = vec![geo_slot];
                view.selected_node = None;
            }
        }
    } else {
        view.current_path = remap(&view.current_path);
    }
}

/// One channel path as it resolved before a move.
struct ChannelRef {
    holder: String,
    param: String,
    path: String,
    target: String,
    absolute: bool,
    param_part: String,
}

/// The parameters whose text spells channel paths: expressions, and a
/// wrangle's Code.
fn spells_paths(p: &crate::app::ParamDef) -> bool {
    p.is_expr() || p.kind() == ParamKind::Code
}

fn channel_refs(root: &FsNode) -> Vec<ChannelRef> {
    fn walk(root: &FsNode, node: &FsNode, out: &mut Vec<ChannelRef>) {
        for p in node.params.iter().filter(|p| spells_paths(p)) {
            crate::expr::rewrite_paths(p.text(), |path| {
                if let Some((target, absolute, param_part)) = crate::geometry::ref_path_target(root, node, path) {
                    out.push(ChannelRef {
                        holder: node.id.clone(),
                        param: p.name.clone(),
                        path: path.to_string(),
                        target,
                        absolute,
                        param_part,
                    });
                }
                None
            });
        }
        for c in &node.children {
            walk(root, c, out);
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// Write each path in `refs` again from where its holder now stands, where
/// the move changed what it has to say.
fn repoint_channel_refs(root: &mut FsNode, refs: &[ChannelRef]) {
    let inside = |root: &FsNode, id: &str| -> bool {
        crate::geometry::node_chain(root, id)
            .and_then(|chain| chain.first().map(|n| is_geometry_container(&n.node_type) && n.id != id))
            .unwrap_or(false)
    };
    let mut edits: Vec<(String, String, String)> = Vec::new();
    for holder in refs.iter().map(|r| r.holder.as_str()).collect::<std::collections::BTreeSet<_>>() {
        let Some(node) = crate::viewer_state::find_node_by_id(root, holder) else { continue };
        for p in node.params.iter().filter(|p| spells_paths(p)) {
            let text = crate::expr::rewrite_paths(p.text(), |path| {
                let r = refs.iter().find(|r| r.holder == holder && r.param == p.name && r.path == path)?;
                let target_moved = inside(root, &r.target);
                let needed = if r.absolute { target_moved } else { inside(root, holder) != target_moved };
                if !needed {
                    return None;
                }
                let node_path = if r.absolute {
                    crate::geometry::absolute_ref_path(root, &r.target)?
                } else {
                    crate::geometry::relative_ref_path(root, holder, &r.target)?
                };
                Some(match node_path.as_str() {
                    "" => r.param_part.clone(),
                    "/" => format!("/{}", r.param_part),
                    np => format!("{np}/{}", r.param_part),
                })
            });
            if text != p.text() {
                edits.push((holder.to_string(), p.name.clone(), text));
            }
        }
    }
    for (holder, param, text) in edits {
        if let Some(n) = crate::viewer_state::find_node_by_id_mut(root, &holder) {
            if let Some(p) = n.params.iter_mut().find(|p| p.name == param) {
                p.set_text(text);
            }
        }
    }
}
