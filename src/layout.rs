//! Auto-layout: arrange a level's nodes from their wiring.
//!
//! The network is already a GRID — every node's position is an integer cell,
//! and the keyboard cursor moves cell by cell — so this is not the usual
//! force-directed sprawl. It is a layered assignment on cells: a node's ROW is
//! how far it is downstream, and its COLUMN is chosen to sit under the node it
//! reads from.
//!
//! **Edges come from the same rule the wires do**: a node's `Input` parameter
//! naming another node. That is the widget's `wire_pairs` derivation, and
//! matching it is the point — a layout computed from relationships you cannot
//! see would move nodes for reasons that are not on screen. It also means a
//! second operand (a Boolean's `With`, a Copy's target) does not pull on the
//! layout, because it does not draw a wire either. When those become wires,
//! they should become edges here in the same change.
//!
//! **Flow is downward**, matching every project in the repo: a Sphere at
//! (4, 2) feeds an output at (4, 3). Row is the LONGEST path from a root, not
//! the shortest, so a node always sits below every one of its inputs rather
//! than beside one of them.
//!
//! Utility nodes are pinned. The settings tree lives at a place the user put
//! it, and an "arrange everything" that relocated the meta node would be a
//! surprise every time. Their cells are treated as occupied so nothing lands
//! on top of them.

/// One node's layout input: what it is called, what it reads, where it is now,
/// and whether it may be moved.
pub struct LayoutNode {
    pub name: String,
    /// The value of its `Input` parameter, if it has one.
    pub input: Option<String>,
    pub position: (f32, f32),
    pub pinned: bool,
}

/// New positions for the nodes that moved, as (index, (column, row)).
///
/// Only movers are returned, so a caller can tell whether the layout changed
/// anything and report it — an arrange that silently did nothing looks broken.
pub fn arrange(nodes: &[LayoutNode]) -> Vec<(usize, (f32, f32))> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }

    // Parent index per node, by the wires' own rule.
    let parent: Vec<Option<usize>> = nodes
        .iter()
        .map(|node| {
            let want = node.input.as_deref()?.trim();
            if want.is_empty() {
                return None;
            }
            nodes.iter().position(|other| other.name == want)
        })
        .collect();

    // Depth by longest path, iteratively. A name-wired graph can contain a
    // cycle (A reads B reads A), and the fixed point below simply stops
    // improving instead of recursing forever — the cycle's members end up at
    // the deepest row any of them could justify, which is as meaningful an
    // answer as a cyclic graph has.
    let mut depth = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for i in 0..n {
            if let Some(p) = parent[i] {
                if p != i && depth[p] + 1 > depth[i] {
                    depth[i] = depth[p] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Cells a pinned node holds; the assignment steps around them.
    let mut taken: Vec<(i32, i32)> = nodes
        .iter()
        .filter(|node| node.pinned)
        .map(|node| (node.position.0 as i32, node.position.1 as i32))
        .collect();

    let max_depth = (0..n).filter(|&i| !nodes[i].pinned).map(|i| depth[i]).max().unwrap_or(0);
    let mut column = vec![0i32; n];
    let mut placed = vec![false; n];
    let mut out = Vec::new();

    for row in 0..=max_depth {
        let mut in_row: Vec<usize> =
            (0..n).filter(|&i| !nodes[i].pinned && depth[i] == row).collect();

        // Order within the row by where the node WANTS to be, so the ordering
        // and the placement agree and the pass does not fight itself. A root's
        // wish is its current column, which preserves the left-to-right
        // arrangement the user already made among independent chains.
        let wish = |i: usize, column: &Vec<i32>, placed: &Vec<bool>| -> i32 {
            match parent[i] {
                Some(p) if placed[p] => column[p],
                _ => nodes[i].position.0.round() as i32,
            }
        };
        in_row.sort_by_key(|&i| (wish(i, &column, &placed), nodes[i].name.clone()));

        for i in in_row {
            let want = wish(i, &column, &placed);
            // Nearest free column to the one it wants, searching outward so a
            // collision nudges a node aside rather than pushing the whole row
            // to the right. A chain whose parent's column is free stays
            // perfectly vertical, which is what a chain should look like.
            // Terminates because `taken` is finite: some column is always free.
            let col = (0i32..)
                .flat_map(|step| {
                    if step == 0 { vec![want] } else { vec![want + step, want - step] }
                })
                .find(|c| !taken.contains(&(*c, row as i32)))
                .expect("an unbounded column scan always finds a free cell");
            taken.push((col, row as i32));
            column[i] = col;
            placed[i] = true;
            let new_pos = (col as f32, row as f32);
            if new_pos != nodes[i].position {
                out.push((i, new_pos));
            }
        }
    }
    out
}
