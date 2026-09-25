//! The command registry — one list of everything the app can be asked to do.
//!
//! Before this there were three vocabularies with nothing holding them
//! together: the [`Action`] enum matched against chords, the menu-item LABELS
//! that `execute_menu_action` dispatches on, and the menubar declarations that
//! put those labels on screen. A command existed in whichever of them someone
//! had needed at the time, so "Show Origin" was an `Action` with no chord, undo
//! was a menu item with no chord, and nothing could tell you either fact.
//!
//! The plugin this app is replacing learned the same lesson and wrote it down:
//! its `hccommands.py` moved the label onto the method as a decorator because
//! a `label -> method` map maintained beside the methods drifted from them, and
//! a renamed method silently emptied the panel. Rust has no decorators, so the
//! equivalent is one static table where each command carries everything about
//! itself, plus a test that every chord and every dispatched label names a row
//! in it. There is still only one place to forget.
//!
//! What a registry entry is NOT is a second implementation. [`Run`] names the
//! path a command already takes — an `Action` or a menu label — so the palette,
//! the chord and the menu all end up in the same code. A command with two ways
//! to run it would be two commands that drift.

use crate::shortcut::Action;

/// Where a command applies.
///
/// The palette ranks by this: commands for the focused pane come first, then
/// everything global. It is not a filter — a pane-specific command is still
/// reachable from anywhere, because a palette that hides what you are looking
/// for is worse than one that lists it second. That is the opposite of the
/// plugin's rule, which drops commands a tab cannot run; the difference is that
/// there every tab genuinely could not run them, while here every pane exists
/// at once and "the viewport's grid" is a thing you may well want from the
/// network editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Always,
    Network,
    Viewport,
    Parameters,
    Spreadsheet,
    Playbar,
}

/// How a command reaches the code that does the work.
///
/// Two variants because the app genuinely has two dispatch paths, and
/// pretending otherwise would mean rewriting one of them to look like the
/// other for no gain. What matters is that a command names exactly one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Run {
    /// Through `State::execute_action` — the typed path, and the only one a
    /// chord could reach before this.
    Key(Action),
    /// Through `State::execute_menu_action` by label — the menubar's path.
    Menu(&'static str),
}

pub struct Command {
    /// Stable and snake_case: this is what `input.kdl` binds and what a
    /// conflict report names. It must not change when the label does — the
    /// case follows that file's existing convention (`zoom_in`,
    /// `close_window`), since being what the user types there is the id's
    /// whole job.
    pub id: &'static str,
    /// What a human reads, in the palette and in the menus.
    pub label: &'static str,
    pub context: Context,
    pub run: Run,
    /// The chord this command has when the user has not said otherwise.
    /// `None` means the command ships unbound and is reachable only through a
    /// menu or the palette — which is a fine thing to be, but now a visible
    /// one.
    pub default_chord: Option<&'static str>,
}

/// Every command, in the order the palette lists ties.
///
/// **A `default_chord` of `None` does not always mean unbound.** The toolkit's
/// runner claims four chords of its own from `input.kdl`'s `cce-ui` domain
/// before an app sees them: `undo` (ctrl+z), `redo` (ctrl+shift+z),
/// `focus_next_group` (ctrl+tab) and `focus_prev_group` (ctrl+shift+tab). Undo
/// and Redo are listed here with no chord for exactly that reason — they are
/// routed to the focused widget FIRST, so a text box undoes its own typing
/// before the app is asked, and registering ctrl+z here would take that away
/// while looking like a fix. The palette still runs them, which is the gain.
///
/// Focus Next/Previous Pane do claim ctrl+tab and ctrl+shift+tab, overriding
/// the runner's group-focus chords. That is deliberate and predates this
/// registry; it is recorded here because `conflicts()` cannot see it — that
/// check compares this app's bindings with each other, not with the toolkit's.
pub const COMMANDS: &[Command] = &[
    // --- File ---
    Command { id: "new_project", label: "New Project", context: Context::Always, run: Run::Menu("New Project"), default_chord: Some("Ctrl+n") },
    Command { id: "open_project", label: "Open", context: Context::Always, run: Run::Menu("Open"), default_chord: Some("Ctrl+o") },
    Command { id: "save_document", label: "Save", context: Context::Always, run: Run::Key(Action::Save), default_chord: Some("Ctrl+s") },
    Command { id: "save_document_as", label: "Save As", context: Context::Always, run: Run::Key(Action::SaveAs), default_chord: Some("Ctrl+Shift+s") },
    Command { id: "set_as_default", label: "Set As Default", context: Context::Always, run: Run::Menu("Set As Default"), default_chord: None },
    Command { id: "exit", label: "Exit", context: Context::Always, run: Run::Menu("Exit"), default_chord: None },

    // --- Edit ---
    // Chordless on purpose: the runner owns ctrl+z / ctrl+shift+z. See above.
    Command { id: "undo", label: "Undo", context: Context::Always, run: Run::Key(Action::Undo), default_chord: None },
    Command { id: "redo", label: "Redo", context: Context::Always, run: Run::Key(Action::Redo), default_chord: None },

    // --- Panes ---
    Command { id: "show_network_pane", label: "Show Network Pane", context: Context::Always, run: Run::Menu("Show Network Pane"), default_chord: None },
    Command { id: "show_viewport_pane", label: "Show Viewport Pane", context: Context::Always, run: Run::Menu("Show Viewport Pane"), default_chord: None },
    Command { id: "show_parameters_pane", label: "Show Parameters Pane", context: Context::Always, run: Run::Menu("Show Parameters Pane"), default_chord: None },
    Command { id: "toggle_spreadsheet", label: "Show Spreadsheet Pane", context: Context::Always, run: Run::Key(Action::ToggleSpreadsheet), default_chord: Some("`") },
    Command { id: "show_playbar_pane", label: "Show Playbar Pane", context: Context::Always, run: Run::Menu("Show Playbar Pane"), default_chord: None },
    Command { id: "close_pane", label: "Close Pane", context: Context::Always, run: Run::Menu("Close Pane"), default_chord: None },
    Command { id: "next_context", label: "Focus Next Pane", context: Context::Always, run: Run::Key(Action::NextContext), default_chord: Some("Ctrl+Tab") },
    Command { id: "previous_context", label: "Focus Previous Pane", context: Context::Always, run: Run::Key(Action::PrevContext), default_chord: Some("Ctrl+Shift+Tab") },
    Command { id: "command_palette", label: "Command Palette", context: Context::Always, run: Run::Key(Action::CommandPalette), default_chord: Some("Ctrl+p") },
    // Alt+D, not Super+D: the compositor claims every Super chord before any
    // client sees one (`input.kdl`'s `cce-window-manager` domain binds
    // super+d to the app launcher), and Super held is also the DE's
    // window-adjust modifier. Alt is the app's own — the network move family
    // already lives there.
    Command { id: "toggle_dialog", label: "Dialog", context: Context::Always, run: Run::Key(Action::ToggleDialog), default_chord: Some("Alt+d") },
    Command { id: "toggle_configure", label: "Configure", context: Context::Always, run: Run::Key(Action::ToggleConfigure), default_chord: Some("Ctrl+,") },

    // Escape already does this, handled inline with the rest of Escape's
    // cascade, so the row ships unbound — it is here to be findable in the
    // palette and bindable by anyone who wants a chord. NOT Ctrl+D, which the
    // plugin uses for deselect-all but which this app already gives to
    // Circular Pane.
    Command { id: "deselect", label: "Deselect", context: Context::Network, run: Run::Key(Action::Deselect), default_chord: None },

    // --- Network navigation ---
    //
    // The plugin's scheme, ported: hjkl rather than arrows (the arrows are the
    // playbar transport in every pane), bare to move the cursor, alt to move
    // the node under it, ctrl to pan the view. `shift+hjkl` — extend the
    // selection — is deliberately absent: the Graph widget carries a single
    // `selected_node`, and a select family with nothing to extend would be
    // four rows that quietly do what bare hjkl already does.
    Command { id: "nav_left", label: "Cursor Left", context: Context::Network, run: Run::Key(Action::NetworkNav(-1, 0)), default_chord: Some("h") },
    Command { id: "nav_down", label: "Cursor Down", context: Context::Network, run: Run::Key(Action::NetworkNav(0, 1)), default_chord: Some("j") },
    Command { id: "nav_up", label: "Cursor Up", context: Context::Network, run: Run::Key(Action::NetworkNav(0, -1)), default_chord: Some("k") },
    Command { id: "nav_right", label: "Cursor Right", context: Context::Network, run: Run::Key(Action::NetworkNav(1, 0)), default_chord: Some("l") },
    // shift+hjkl — the plugin's extend-the-selection family. It was absent
    // while the graph's single `selected_node` was the whole selection; the
    // cursor is a REGION now, so these grow its far corner and the nodes
    // inside it are the selection (see `State::selected_slots`).
    Command { id: "extend_left", label: "Extend Selection Left", context: Context::Network, run: Run::Key(Action::NetworkExtend(-1, 0)), default_chord: Some("Shift+h") },
    Command { id: "extend_down", label: "Extend Selection Down", context: Context::Network, run: Run::Key(Action::NetworkExtend(0, 1)), default_chord: Some("Shift+j") },
    Command { id: "extend_up", label: "Extend Selection Up", context: Context::Network, run: Run::Key(Action::NetworkExtend(0, -1)), default_chord: Some("Shift+k") },
    Command { id: "extend_right", label: "Extend Selection Right", context: Context::Network, run: Run::Key(Action::NetworkExtend(1, 0)), default_chord: Some("Shift+l") },
    Command { id: "move_left", label: "Move Node Left", context: Context::Network, run: Run::Key(Action::NetworkMove(-1, 0)), default_chord: Some("Alt+h") },
    Command { id: "move_down", label: "Move Node Down", context: Context::Network, run: Run::Key(Action::NetworkMove(0, 1)), default_chord: Some("Alt+j") },
    Command { id: "move_up", label: "Move Node Up", context: Context::Network, run: Run::Key(Action::NetworkMove(0, -1)), default_chord: Some("Alt+k") },
    Command { id: "move_right", label: "Move Node Right", context: Context::Network, run: Run::Key(Action::NetworkMove(1, 0)), default_chord: Some("Alt+l") },
    Command { id: "view_left", label: "Pan View Left", context: Context::Network, run: Run::Key(Action::NetworkPan(-1, 0)), default_chord: Some("Ctrl+h") },
    Command { id: "view_down", label: "Pan View Down", context: Context::Network, run: Run::Key(Action::NetworkPan(0, 1)), default_chord: Some("Ctrl+j") },
    Command { id: "view_up", label: "Pan View Up", context: Context::Network, run: Run::Key(Action::NetworkPan(0, -1)), default_chord: Some("Ctrl+k") },
    Command { id: "view_right", label: "Pan View Right", context: Context::Network, run: Run::Key(Action::NetworkPan(1, 0)), default_chord: Some("Ctrl+l") },
    Command { id: "frame_cursor", label: "Frame Cursor", context: Context::Network, run: Run::Key(Action::FrameCursor), default_chord: Some("f") },
    // Ctrl+Shift+L rather than the L that Houdini uses: bare hjkl is the
    // cursor, and shift+hjkl is reserved for the select family this app cannot
    // implement yet — taking Shift+L now would have to be given back later.
    Command { id: "layout_nodes", label: "Layout Nodes", context: Context::Network, run: Run::Key(Action::LayoutNodes), default_chord: Some("Ctrl+Shift+l") },
    Command { id: "frame_all", label: "Frame All", context: Context::Network, run: Run::Key(Action::FrameAll), default_chord: Some("Shift+f") },

    // --- Network ---
    // The add-node palette. Tab opens it inline (like Escape's cascade, and
    // like `deselect` above, the row ships unbound rather than duplicating a
    // key the event loop already claims), and it is the first row of the
    // network's right-click menu, which dispatches through this id.
    Command { id: "add_node", label: "Add Node", context: Context::Network, run: Run::Menu("Add Node"), default_chord: None },
    Command { id: "zoom_in", label: "Zoom In", context: Context::Network, run: Run::Menu("Zoom In"), default_chord: None },
    Command { id: "zoom_out", label: "Zoom Out", context: Context::Network, run: Run::Menu("Zoom Out"), default_chord: None },
    Command { id: "reset_zoom", label: "Reset Zoom", context: Context::Network, run: Run::Menu("Reset Zoom"), default_chord: None },
    Command { id: "toggle_network_plate", label: "Network Plate", context: Context::Network, run: Run::Key(Action::ToggleNetworkPlate), default_chord: Some("Shift+p") },
    Command { id: "toggle_circular_pane", label: "Circular Pane", context: Context::Network, run: Run::Key(Action::ToggleCircularPane), default_chord: Some("Ctrl+d") },
    Command { id: "detach_circular_window", label: "Detach Circular Window", context: Context::Network, run: Run::Key(Action::DetachCircularWindow), default_chord: None },

    // --- Viewer states ---
    // Ctrl+Shift+H, not Ctrl+H: the ctrl+hjkl family below is the network
    // pane's view panning, and the conflict check caught the collision the
    // first time both existed.
    Command { id: "edit_handles", label: "Edit Handles", context: Context::Viewport, run: Run::Key(Action::ToggleViewerState), default_chord: Some("Ctrl+Shift+h") },
    Command { id: "toggle_snap", label: "Toggle Snapping", context: Context::Viewport, run: Run::Key(Action::ToggleSnap), default_chord: Some("Ctrl+b") },

    // --- Viewport ---
    Command { id: "toggle_grid", label: "Show Grid", context: Context::Viewport, run: Run::Key(Action::ToggleGrid), default_chord: Some("Ctrl+g") },
    Command { id: "toggle_cube", label: "Show Cube", context: Context::Viewport, run: Run::Key(Action::ToggleCube), default_chord: Some("Ctrl+e") },
    Command { id: "toggle_origin", label: "Show Origin", context: Context::Viewport, run: Run::Key(Action::ToggleOrigin), default_chord: None },
    Command { id: "toggle_camera_pivot", label: "Show Camera Pivot", context: Context::Viewport, run: Run::Key(Action::ToggleCameraPivot), default_chord: None },
    Command { id: "toggle_wireframe", label: "Show Wireframe", context: Context::Viewport, run: Run::Key(Action::ToggleWireframe), default_chord: None },
    Command { id: "toggle_smooth_shading", label: "Smooth Shading", context: Context::Viewport, run: Run::Key(Action::ToggleSmoothShading), default_chord: None },
    Command { id: "toggle_show_occluded", label: "Show Occluded", context: Context::Viewport, run: Run::Key(Action::ToggleShowOccluded), default_chord: None },
    // The point overlays on the visible scene. Per-node `meta` child
    // preferences until 2026-09-23; global display settings now, reached
    // here like every other viewport toggle.
    Command { id: "toggle_point_markers", label: "Show Point Markers", context: Context::Viewport, run: Run::Key(Action::TogglePointMarkers), default_chord: None },
    Command { id: "toggle_point_numbers", label: "Show Point Numbers", context: Context::Viewport, run: Run::Key(Action::TogglePointNumbers), default_chord: None },
    Command { id: "toggle_point_normals", label: "Show Point Normals", context: Context::Viewport, run: Run::Key(Action::TogglePointNormals), default_chord: None },
    Command { id: "toggle_render_points", label: "Show Points", context: Context::Viewport, run: Run::Key(Action::ToggleRenderPoints), default_chord: None },
    Command { id: "toggle_wire_single_color", label: "Wireframe Single Color", context: Context::Viewport, run: Run::Key(Action::ToggleWireSingleColor), default_chord: None },
    Command { id: "toggle_ray_traced_preview", label: "Ray Traced Preview", context: Context::Viewport, run: Run::Key(Action::ToggleRayTracedPreview), default_chord: None },
    Command { id: "toggle_square_viewport", label: "Square Aspect", context: Context::Viewport, run: Run::Key(Action::ToggleSquareViewport), default_chord: Some("Ctrl+a") },

    // --- Parameters ---
    Command { id: "export", label: "Export", context: Context::Parameters, run: Run::Menu("Export"), default_chord: None },

    // --- Playbar ---
    Command { id: "play_pause", label: "Play / Pause", context: Context::Playbar, run: Run::Key(Action::PlayPause), default_chord: Some("Up") },
    Command { id: "play_pause_reverse", label: "Play / Pause Reverse", context: Context::Playbar, run: Run::Key(Action::PlayPauseReverse), default_chord: Some("Down") },
    Command { id: "frame_next", label: "Next Frame", context: Context::Playbar, run: Run::Key(Action::FrameNext), default_chord: Some("Right") },
    Command { id: "frame_prev", label: "Previous Frame", context: Context::Playbar, run: Run::Key(Action::FramePrev), default_chord: Some("Left") },
    // Ctrl+Up rewinds: stops a moving timeline and lands on the start frame,
    // the way a transport's stop-to-start does — one press whatever the
    // timeline is doing.
    Command { id: "frame_start", label: "Go To Start Frame", context: Context::Playbar, run: Run::Key(Action::FrameStart), default_chord: Some("Ctrl+Up") },
];

pub fn by_id(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// A chord claimed by more than one command.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub chord: String,
    /// The command that wins — the earlier one in [`COMMANDS`], because
    /// matching is first-wins over registration order.
    pub winner: &'static str,
    /// The commands that are consequently unreachable by this chord.
    pub shadowed: Vec<&'static str>,
}

/// Which chords two or more commands claim, given each command's RESOLVED
/// chord (the default, or the user's override from `input.kdl`).
///
/// Worth detecting because the failure is silent and looks like a broken
/// command rather than a broken binding: `match_action` returns the first
/// match, so the second command simply never runs and says nothing about why.
/// Chords are compared as parsed, not as text, so "Ctrl+S" and "ctrl+s"
/// collide the way they actually do at the keyboard.
pub fn conflicts(resolved: &[(&'static str, String)]) -> Vec<Conflict> {
    let mut seen: Vec<(crate::shortcut::Shortcut, String, &'static str, Vec<&'static str>)> =
        Vec::new();
    for (id, chord) in resolved {
        let Ok(parsed) = crate::shortcut::Shortcut::parse(chord) else { continue };
        match seen.iter_mut().find(|(s, _, _, _)| *s == parsed) {
            Some((_, _, _, shadowed)) => shadowed.push(id),
            None => seen.push((parsed, chord.clone(), id, Vec::new())),
        }
    }
    seen.into_iter()
        .filter(|(_, _, _, shadowed)| !shadowed.is_empty())
        .map(|(_, chord, winner, shadowed)| Conflict { chord, winner, shadowed })
        .collect()
}

/// Rank `haystack` against a fuzzy `query`, returning the matching indices
/// best first.
///
/// The ranking is the plugin's fuzzyfinder, deliberately: shortest contiguous
/// span containing the query's characters in order, then earliest start, then
/// alphabetical. Keeping the ranking means muscle memory survives the move —
/// typing "sg" has to keep landing on Show Grid.
///
/// An empty query matches everything, in registry order, which is what makes
/// the palette usable as a plain list of what exists.
pub fn fuzzy_rank(query: &str, haystack: &[&str]) -> Vec<usize> {
    let needle: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return (0..haystack.len()).collect();
    }
    let mut scored: Vec<(usize, usize, &str, usize)> = Vec::new();
    for (i, item) in haystack.iter().enumerate() {
        let chars: Vec<char> = item.to_lowercase().chars().collect();
        // The shortest window containing the needle as a subsequence: try each
        // start, take the first that matches, keep the tightest. The plugin
        // gets this from an overlapping-match lookahead regex; the loop is the
        // same answer without the regex engine.
        let mut best: Option<(usize, usize)> = None;
        for start in 0..chars.len() {
            if chars[start] != needle[0] {
                continue;
            }
            let mut n = 1;
            let mut end = start + 1;
            while end < chars.len() && n < needle.len() {
                if chars[end] == needle[n] {
                    n += 1;
                }
                end += 1;
            }
            if n == needle.len() {
                let span = end - start;
                if best.is_none_or(|(b, _)| span < b) {
                    best = Some((span, start));
                }
            }
        }
        if let Some((span, start)) = best {
            scored.push((span, start, item, i));
        }
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    scored.into_iter().map(|(_, _, _, i)| i).collect()
}

/// One palette row: the label padded to `width`, then its chord.
///
/// Padded rather than tab-separated because the popup renders a tab as a
/// single literal tab stop, so chords after labels of different lengths do not
/// line up into a column.
pub fn palette_row(label: &str, chord: Option<String>, width: usize) -> String {
    match chord {
        Some(chord) => format!("{label:<width$}{chord}"),
        None => label.to_string(),
    }
}

/// The command a palette row names.
///
/// The LONGEST label the row starts with, because labels prefix each other:
/// "Save" starts the row that belongs to "Save As", and picking the first
/// match would run the wrong command from a padded row.
pub fn from_palette_row(row: &str) -> Option<&'static Command> {
    let row = row.trim_end();
    COMMANDS.iter().filter(|c| row.starts_with(c.label)).max_by_key(|c| c.label.len())
}

/// The commands to offer, ranked: `query` decides which, `focused` decides the
/// order among equals.
pub fn palette_entries(query: &str, focused: Context) -> Vec<&'static Command> {
    let labels: Vec<&str> = COMMANDS.iter().map(|c| c.label).collect();
    let contexts: Vec<Context> = COMMANDS.iter().map(|c| c.context).collect();
    rank_with_focus(query, &labels, &contexts, focused).into_iter().map(|i| &COMMANDS[i]).collect()
}

/// `fuzzy_rank` over `labels`, then the entries whose context is the
/// focused pane's partitioned to the front — stably, so the fuzzy ranking
/// survives inside each half, and without dropping anything (a palette that
/// hides what you are looking for is worse than one that lists it second).
/// The dialog ranks its setting rows alongside the commands through this,
/// as `Context::Always` entries.
pub fn rank_with_focus(query: &str, labels: &[&str], contexts: &[Context], focused: Context) -> Vec<usize> {
    let mut ranked = fuzzy_rank(query, labels);
    if focused != Context::Always {
        let (mine, rest): (Vec<_>, Vec<_>) = ranked.into_iter().partition(|&i| contexts[i] == focused);
        ranked = mine;
        ranked.extend(rest);
    }
    ranked
}
