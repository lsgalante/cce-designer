# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-designer` is a node-based procedural 3D design app (Houdini-style) for the cce
desktop environment: a node graph is evaluated into geometry — native Rust
operators, plus a Rhai wrangle for per-element scripting — and displayed in a 3D viewport with both a raster pass and a path-traced
(RT) preview mode.

This crate is one member of the multi-repo `cce` Cargo workspace; workspace-wide rules
(multi-repo layout, no `[workspace.dependencies]`, shared `../target/`) live in
`../cce-compositor/WORKSPACE.md`. This directory is its own git repository.

## Build, test, run

```sh
cargo build -p cce-designer            # from the workspace root
cargo run  -p cce-designer             # needs a Wayland session (cce or any compositor)
cargo test -p cce-designer             # all tests live in src/main.rs's tests module
cargo test -p cce-designer test_keyboard_shortcut_system   # one test
make install                           # release build, then `ccebuild install --no-build cce-designer`
```

Two binaries: `cce-designer` (the app) and `vk-smoke` (`src/vk_smoke.rs`) — a
standalone renderer smoke test that opens its own window; run it inside a Wayland
session with `cargo run -p cce-designer --bin vk-smoke`.

The suite is pure CPU and needs no GPU, no OpenCL and no Wayland. (Until
2026-09-24 it ran node kernels through the OpenCL runtime when one existed,
with a CPU interpreter as the headless fallback, and `CCE_KERNEL_CPU=1` was
the reliable way to run it — see "OpenCL is retired" below for why that is
gone.)

### CLI modes

- `cce-designer --thumbnail <project-dir-or-state.json> <out.png> [--size N] [--samples N] [--frame N]`
  — headless path-traced thumbnail (no Wayland, no window; `src/thumbnail.rs`).
  `cce-files` shells out to this for its preview cache. Without `--frame` there
  is no timeline and simnets render at their seed; with it the solve runs to
  that frame (start frame 1, the playbar's default), which is the only way to
  look at a simulation without a Wayland session.
- `cce-designer --export <project> <out.stl|out.obj> [--frame N] [--node NAME] [--scale S]`
  — headless mesh export (`src/export_cli.rs`, formats in `src/export.rs`).
  The format comes from the extension, defaulting to binary STL. Without
  `--node` the whole visible scene is written; with it, that one node's output
  is written whether or not it is visible, which is normal for an Export node
  whose input something else already draws. Same frame contract as
  `--thumbnail`.
- `cce-designer --detached-network` — a separate network-pane-only window. It syncs
  with the main window by autosaving/polling `default_project.json` mtime (see the
  main loop in `src/main.rs`) — there is no socket between the two.
- `cce-designer --detached-params` / `--detached-spreadsheet` / `--detached-playbar`
  — the same idea for the other plates (`plate_corner::pane_detach_flag`), spawned by
  the plate corner menu's Detach. These windows are plain rectangles with standard CSD
  and no 3D canvas; `--detached-network` stays its own flag because that window is
  CIRCULAR, with a radial border resize no rectangular pane wants. All of them share
  the one `default_project.json` sync channel, and only the main window runs the MCP
  server. The parent keeps a stub for each pane it handed out — that stub's corner
  control is the only way to Reattach — and reaps its children with `try_wait` from
  the frame tick, so a window the user closes hands its pane back. NOT `kill(pid, 0)`:
  an unreaped exited child is a zombie, which that probe calls alive forever.
  The main window's reload of that channel takes the TREE and navigation only
  (`load_sync_channel(path, keep_own_view: true)`), never the view state a
  detached window wrote — that is the detached window's defaults, and applying
  it reset the main window's plate sizes, panes and camera on every autosave.
  The write side matches: a detached window's save keeps the view state
  already in the file and replaces only the navigation (`detached_view_state`),
  so the file always carries the main window's layout.
  Note that detaching REWRITES `default_project.json` in the source tree, since that
  file is the sync channel; it is versioned, so check `git status` after testing.
  **A detached pane window is a working satellite only since 2026-09-29.**
  Two things were missing for every pane but the circular network. The
  window never read the channel at startup: `State::new` seeds the tree,
  camera name, pan and path from the bundled file and records its mtime, so
  the window waited for a change the file it had just been handed was never
  going to have, and a detached parameters window opened with nothing
  selected — an empty pane (`seed_detached_window` takes the channel whole
  now). And the two places that ASK for an autosave named only the circular
  network, so with the parameters, spreadsheet or playbar detached neither
  window wrote the channel again after the detach; `State::syncing_windows`
  is the one test the requests, the poll and the exit save share.
  `a_detached_params_window_follows_the_selection_and_the_camera` covers
  both, and the camera that rides the same channel.

### MCP automation server

The main window runs an embedded MCP server on `127.0.0.1:3001`
(`CCE_DESIGNER_MCP_PORT` overrides so a second instance can run alongside;
`src/api.rs`). This is the way to drive/inspect the running app: attach with
`claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`, or
speak JSON-RPC directly with curl (`initialize` / `tools/list` / `tools/call`).
There is one tool per `McpAction` variant (tool name = the variant's serde
tag, dispatched in `apply_mcp_call` in `src/window.rs`) plus `get_state`,
which returns the project beside three app-state fields that are not part of
the save: `playbar`, `grid` and `status` — the status line's text as shown,
which is the way to read a load report, a node error or a refused edit when
the window is off-screen. The
tool list lives in `mcp_tools()` in `src/api.rs`; the protocol layer is
`cce_ui::mcp` (tools-only Streamable HTTP). Keep the enum, the tool list, and
the schemas in sync — `test_mcp_tools_map_to_actions` enforces the mapping.
**`menu_click` dispatches three menus and refuses the rest** (since
2026-09-29). The five menubars are roster slots that are never drawn —
`HEADER_H` and `MENUBAR_H` are 0 — kept as pane identities and for their
checkmarks, so the MCP tool is the only thing that can click one. What
`process_window_event` still dispatches by index is what no registry command
was then: the viewport menubar's Camera menu, and the parameters menubar's
Preset and Reset (`window::menu_is_dispatched`). Everything else they list is
a command, reached through `run_command`; a click on it is an error saying so,
where it used to be accepted and, the item lists having drifted from the
matches, ran the wrong item (the header's Save opened a project).
`a_menubar_click_is_dispatched_or_refused` is the test.

**The cameras and the parameter reset are commands too** (the same day),
so the palette reaches them and a chord can: `next_camera` /
`previous_camera` step through `State::camera_names` (the Default Camera,
then the level's camera nodes) and wrap, `default_camera` goes back to it,
and each camera NODE is a row of the palette — `Camera: camera1`, id under
`CAMERA_ROW_PREFIX`, ranked among the commands as the viewport's, with
`active` in the chord column of the one in use. They are rows and not
registry commands because they are nodes, as the recent projects are paths.
`State::choose_camera` is the one entry. `reset_parameters` runs
`State::reset_parameters` on the node the params pane shows, from
`template_default` — so a child inside a subnet instance resets to the
subnet template's override, where the menubar's arm looked the template up
by node type and missed it. All four ship unbound, and the three menubar
menus run the same functions. **The Custom preset is retired**: it was the
template's defaults with their numbers scaled by half again, a stand-in
that stored nothing and read nothing of the node's own values.
`the_cameras_and_the_parameter_reset_are_commands` is the test.

**Edits to the node tree are undoable** (`src/edit_history.rs`, the same
day), the first thing outside a text box or a viewer state that is.
`State::edit_history` is ONE stack of two kinds of step, so that undo takes
them back in the order they were made:

- **Parameters**: the parameters of one node that CHANGED, as they stood.
  Recorded by the writers, through `State::record_params` — the params
  pane's write-back (`sync_parameters_to_project`), the row menu
  (`run_param_action`, whose work is `run_param_action_unrecorded`), MCP's
  `set_param`, and Reset Parameters.
- **Structure**: nodes added, removed, moved and renamed, wires made and
  broken, the display and bypass flags. Recorded by NOTICING:
  `record_structure_changes` compares the tree with how it stood at the
  last look (`State::structure_base`) and what differs is the step. It
  runs at the end of `process_window_event` and of `apply_action`, and
  ahead of every undo and every parameter record, so nothing done is left
  unlooked at. There are a dozen writers of the graph — the widget's drag
  read back by `read_panel_offsets`, the keyboard families, paste, the
  palette's pick, MCP, the image commands, Arrange — and a recording call
  in each is one the thirteenth would not make. **A new writer of the
  graph needs nothing.**

The rules:

- **A step holds what changed and nothing else.** A parameter step names
  its parameters; a structure step names its nodes, by id, and of a node
  that stayed only its position, its two flags and its wires (the
  parameters of the `node` kind). So what is written without being
  recorded is left as it is by an undo: a camera node's Rotation under an
  orbit, a curve's Points under its handles (which have their own history
  while the viewer state lasts), View 1:1, the template merge. A removed
  node is kept whole, children and all, and comes back at the place it had
  among its siblings.
- **What replaces the tree is not an edit.** New Project and Open clear
  the history and drop the base; the sync channel's reload drops the base
  and keeps the history. A level whose children cannot be told apart by id
  (a hand-built tree with empty ids) is not followed.
- **One step per gesture.** Records that share a group are one step until
  the group is broken: by any press or release, by Enter, Tab or Escape
  (all at the top of `handle_event`), or by `GROUP_IDLE` (a second) with
  nothing recorded. The pane's group is the node and the parameters
  changed; the graph's is a MOVE of the same nodes, so a run of alt+hjkl is
  one step. The graph is not looked at while a drag is held
  (`gesture_held`), so a dragged node is one step from where it was picked
  up.
- **Slots are held by id across a structure step**: the two editors'
  paths and the selection, which a node coming or going would move. An
  editor inside a node that an undo takes out comes up to where the node
  was.
- **Undo and Redo consult it LAST** — a code row, then a viewer state,
  then this (`Application::undo` for the chord, `Action::Undo` for the
  palette) — so the order ACROSS the three is by owner and not by time. A
  focused text box is ahead of all of them, in the toolkit's runner.
- **A rename is put back by renaming.** A structure step holds the
  nodes renamed as (id, the name it was), and `restore` runs
  `rename_node_in_tree` on each, so the wires and the expression paths
  that name the node, anywhere in the tree, are written back with it —
  writing the name alone would leave them naming nothing. The active
  camera is held by id across the step. The names go back AFTER the wires
  the step holds: the other way round, what is filed for redo is those
  wires as the rename had just left them, and redo puts the old name on
  them. A name a sibling has taken since is not taken twice; the node
  keeps the one it has.
- It is not cce-ui's `History` because that has no way to look at a step
  before taking it, and what is filed for redo is the current state of
  what the step names.

`the_graph_is_undone_a_step_at_a_time`,
`a_rename_is_undone_with_what_names_the_node`,
`a_parameter_edit_is_undone_a_gesture_at_a_time` and
`reset_parameters_is_undone_and_redone` are the tests.

(The former bespoke HTTP API on port 3000 was retired in favor of this;
app-internal threads like the cce-files choosers now return results via
`CustomEvent::RunAction` instead of POSTing to it.)

## Architecture

The designer runs on cce-ui's standard `Application` trait / `engine::run` pattern
(`src/application.rs` holds the impl; the engine owns the Wayland plumbing, calloop
loop, and the `VkRenderer`). Because it draws a 3D scene and shapes its own text, it
uses the engine's extended hooks — it is the reference consumer for them:
`renderer_init` (create persistent meshes), `stage_renderer` (flush pending mesh
updates, stage the raster scene / RT pane; returns true while the path tracer
refines), `handle_resize`, and `standard_csd` / `cursor_icon` / `take_window_action`
(the detached circular window's radial border resize + top-arc move). The 2D frame —
geometry AND text — is the engine's single paint path: `display_list` returns
`State::collect_display_list()` and `display_list_text` opts the text into the
engine's shaping/glyph pass (the app has no `FontSystem` or buffer cache of its own
— the standalone `vk-smoke` bin is the one place that keeps its own, reached through
`cce_ui::cosmic_text`; `glyphon` is not a dependency of this crate at all, having
gone from cce-ui with the wgpu path).

- `src/app.rs` (~10.8k lines) — the heart: `State` (the entire app model), `McpAction` /
  `CustomEvent`, node-template loading, pane layout. `tick_frame` (simulation:
  config polling, inertia, widget ticks) and `stage_frame` (renderer staging) are the
  two halves of the old render loop. GPU mesh updates are staged CPU-side
  (`pending_*` fields, `spheres_dirty`) and flushed in `stage_frame` because only
  the engine hooks see the renderer.
- `src/slots.rs` — the widget roster. Top-level widgets live in fixed slots on
  `WidgetSlots` addressed by `*_IDX` constants (`VIEWPORT_IDX`, `PARAM_IDX`,
  `NETWORK_PANEL_IDX`, … up to `WIDGET_COUNT`) rather than a dynamic tree; every slot
  is statically typed, and index-driven paths (draw order, focus cycling, broadcast
  loops) go through `get_dyn`/`get_dyn_mut`. The roster is declared once, as one line
  per slot in the `widget_roster!` macro invocation (`INDEX_CONST: field: WidgetType`),
  which generates the constants, `WIDGET_COUNT`, the struct fields and all four
  dispatch matches — adding a pane is that one line. The typed accessors that assert a
  slot's concrete type (`viewport()`, `graph_mut()`, `menu(idx)`, …) live here too, and
  `State` keeps one-line forwarders. `PassivePlate` and `Canvas`, the two app-owned
  slot-only widgets, are also here.
- `src/dialog.rs` — the Alt+D dialog: the `Dialog` widget (a third app-owned
  slot-only widget) plus the `State` half that fills it, routes its input and
  writes its settings back. See "The dialog (Alt+D)" below — three of its four
  hard parts are about paint order and occlusion, none of which is guessable
  from the widget.
- `src/plate_corner.rs` — the plate corner control: a circular menu trigger on the
  top-right of each pane that draws its own plate (`PLATE_SLOTS` — network, params,
  spreadsheet, playbar; NOT the viewport, whose plate is the window-spanning lip).
  Geometry is derived from the slot's live rect, so it holds across all three
  `rebuild_positions` branches; the circular network pane is special-cased onto its
  arc. The menu is a fourth `cce_ui::widget::context_menu` consumer alongside the node,
  viewport and network right-click menus, with the same `*_menu_actions` +
  `handle_*_menu_click` contract. Collapse shrinks a plate to its title stub via `apply_collapsed_panes`, a
  post-pass over `positions[..]` (one place, all three branches); the stub is exempt
  from the minimum-span guard or it would lose the control that expands it again.
- `src/application.rs` — the `Application` impl: translates engine hooks into
  `WindowEvent`s, detached-window CSD, HTTP-server startup, exit autosave.
- `src/window.rs` — `WindowEvent` plus the post-event side-effect pass
  (`process_window_event`: menu clicks, pane toggles) and HTTP-action application
  (`apply_custom_event`).
- `src/render.rs` — `State::collect_display_list`: the frame's 2D content as one
  `cce_ui::scene::paint::DisplayList` (prims + `Prim::Text`), hand-maintained draw
  order over the widget slots, circular-pane clipping via `PaintItem::clip_circle`,
  network fade via text alpha. Rebuilt every drawn frame; the engine tessellates,
  shapes, and draws it.
- `src/geometry.rs` — node-graph evaluation. Every evaluator threads an
  `EvalSim` (current frame + `SimCache` + feedback stack) alongside the error
  slot. The `simnet` node type iterates: the chain between its `input` and
  `output` children is one simulation STEP; step 1 eats the simnet's own
  `Input` (like a subnet), each later step eats the previous state, which the
  `input` node reads off the feedback stack instead of jumping to the outer
  graph. Solves run up to the playbar frame and cache per node id on `State::
  sim_cache` (playing forward = one step per frame); the cache key hashes the
  simnet subtree + seed, so edits restart the sim, and backward scrubs resume
  from the nearest CHECKPOINT behind them (steps are not invertible; until
  2026-09-29 they restarted from the seed — see "Simulation checkpoints"
  below). The scene walk does NOT recurse
  into a simnet's children — that would draw one un-iterated pass of the chain
  on top of the solved result. Dived INTO a simnet, the output child's
  geometry flag draws the solved state, and every OTHER visible child draws
  itself as the current frame's LAST SUBSTEP saw it, with the feedback stack
  holding the state that substep consumed (`simnet_step_feedback`, read off
  the `SimSolve` the solve already keeps): `input` shows what the pass
  reads, a chain node shows the pass that landed on the displayed state —
  so the chain's last mover draws where the output draws — and a node not
  wired into the chain at all simply draws. Until 2026-09-28 the feedback
  was the frame's STARTING state, so with four substeps a chain node drew
  three substeps behind the output, which read as the interior lagging a
  frame. Until 2026-09-21 only the output flag drew, and a visible
  node inside a simnet was a node you could not see. At any
  displayed level `input`/`output` children draw their resolved geometry (top
  level of the walk only, so outer views don't draw subnet chains twice).
  Frame changes invalidate the scene only when the
  graph `contains_simnet`. `network_sphere_vertices_with_errors` walks the
  graph from output nodes; node failures are collected into one error slot
  (still named `ocl_error` from the days it held OpenCL's), not fatal.
- `src/viewport_3d.rs` — app-owned `Viewport3D` widget (camera orbit/zoom, inertial
  scroll, `rt_mode` flag switching the pane to the `cce_ui::vk` compute path tracer).
- `src/viewer_state.rs` — the **viewer-state framework**: interactive viewport
  tools, generalized out of the curve tool. A viewer state is a mode the
  viewport is in, bound to one node, in which the pointer edits that node
  instead of orbiting the camera. The framework owns everything that turned out
  to be the same for any such tool: projection of world positions to handles
  through `State::last_scene_mvp` + `last_scene_view_rect` (both LOGICAL px,
  the rect divided by scale where it is cached — the same path as the
  Point Numbers overlay), hit-testing against `cursor_x/y`, dragging by
  unprojecting the cursor at the grabbed handle's captured NDC depth, snapping,
  the HUD, per-gesture undo (`cce_ui::history::History` of handle snapshots on
  the tool, so it lives exactly as long as the state does), binding by node ID
  rather than slot so renames don't detach it and a vanished node drops the
  state lazily, and write-back through the SetParam resync sequence
  (`sync_nodes` + `rebuild_scene_geometry` + `sync_parameters_pane`).
  Input hooks live in `handle_event`: presses intercept in the MouseInput arm
  ahead of the viewport context menu (gated on `cursor_in_viewport() &&
  !in_network_pane`, so the network plate keeps its clicks where they overlap),
  motion at the top of CursorMoved, Escape ahead of connection-cancel.

  What differs per tool is the `HandleSource` trait: which node types it
  accepts, where the handles are, how to write them back, whether the pointer
  may add and remove them, and what to label them. `source_for` is the one map
  from node type to tool, so the node context menu's Edit Handles entry, the
  `edit_handles` command and any future entry point cannot disagree about what
  is editable — adding a source makes it appear in the menu without touching
  the menu.

  Four implementations ship. The first two are deliberately different in shape, because an
  abstraction with a single implementation has not been shown to be one:
  `src/curve_tool.rs` (an open-ended list of world positions in the `curve`
  node's Points parameter, extensible) and `src/soft_transform_tool.rs` (a
  FIXED pair where the second handle is `Centre + Translation` — a derived
  position that has to be converted both ways, which is exactly what the trait
  exists to contain). The soft transform's two handles read as a vector with a
  base and a tip, and dragging either end changes the offset between them; a
  rule like "keep the translation when the centre moves" would be right for the
  drag and would quietly discard half of every restored undo snapshot, since
  `write` is handed a full set of handles with no word about which moved.

  **Three hooks were added for handles that are not world positions of
  their own** (2026-09-29, for the image tools). `read` and `write` take a
  `HandleCtx` — the `PageFrame` of the page the node draws on, and what a
  world unit is — gathered by the framework ahead of the call, because
  `write` holds the node mutably and can look nothing up. `drag(handles,
  moved, to)` is what the whole set is after one handle moves: moving the
  one is the default, and a source whose handles hang off one another
  carries them there (a shape's corner goes with its middle), so `write`
  is still handed a whole set that means one thing whether it came from a
  drag or an undo snapshot — the objection the soft transform's pair
  raises against "keep the translation when the centre moves" does not
  arise, since the rule is applied to the handles and not inside `write`.
  `plane` is the plane the handles live in: a drag then follows the
  cursor's ray to it, where without one it goes to the camera-facing
  plane at the grab depth, which leaves a flat thing's plane as soon as
  the view is not square to it. Two more are for the overlay: `outline`,
  a closed loop drawn under the handles, and `cage`, whether the handles
  are joined in order. Handle labels draw on a dark tab, since a handle
  can stand over a white image.

  The HUD draws one line ABOVE the scale readout, sharing its left margin — not
  at the top, because the viewport is full-bleed and the pane plates float over
  its top edge, so a mode line there lands under the collapsed stubs. It exists
  because a viewer state changes what every click does and snapping silently
  changes what a drag does.
- `src/param.rs` — node parameters: `ParamDef` (text and parsed value kept
  together, both private), `ParamKind`, `ParamValue`, `ParamSlot`. See
  "Parameter kinds and typed values".
- `src/project.rs` — save/load. A project is a **directory containing `state.json`**
  (`Project { name, root: FsNode, view_state }`); `default_project.json` in the crate
  root is special-cased as a single file and doubles as the detached-window sync channel.
- `src/shortcut.rs` — `Shortcut::parse("Ctrl+Shift+g")` and chord → COMMAND ID
  matching (see the command registry above; a chord names a row in
  `src/command.rs`, not an `Action`). `Shortcut`'s equality is hand-written
  rather than derived, so it agrees with `matches` about case.

The `zcce_inspector_v1` integration (window-position tracking + widget-state
streaming to cce-test-interface) was dropped in the engine migration; the HTTP API
is the introspection surface.

### There are no meta nodes (retired 2026-09-23)

Two different things were called `meta`, and both are gone. What replaced
them is the one rule worth remembering: **a display setting is a live
field on `State`, persisted to `state.kdl`, and reached from the command
palette.** Never a node. (Since 2026-09-24 a project ALSO carries the
display settings it was saved with — see "Display settings ride the
project file" below; that is a snapshot in the view state, not a node.)

**The root `meta` node (nee Session)** was a permanent, undeletable root
subnet holding four utility subnets — `main`, `view`, `guides`, `render` —
whose params were every session-wide setting. It was the STORE OF RECORD:
`ensure_menubar_subnets` rebuilt it from live state and
`apply_settings_from_menubar_subnets` copied it back OVER live state after
every parameter edit anywhere. Three things followed, all bad. A display
preference was project data, carried in the file and reset by opening
someone else's scene. Half of those settings were reachable only by finding
the right node in the right subnet. And a command that flipped a live flag
was undone by the next unrelated edit unless it also wrote the node — which
is what `write_guides_toggle` / `write_render_toggle` existed for, and what
made "Show Cube hides the cube until you touch any parameter" a real bug.

**The per-node `meta` child** was a hidden child on every geometry node
carrying four display switches (Point Markers, Point Numbers, Point Normals,
Wireframe), so seeing the point numbering of what was on screen meant diving
into each node and flipping its own switch, one node at a time. Wireframe
was already duplicated by a global `toggle_wireframe`.

Where it all went:

- **Display settings** are live `State` fields, persisted by
  `DesignSettings` into `state.kdl` (`viewport` and the new `render` block),
  and edited as rows of the dialog's one list — `SETTINGS` in
  `src/dialog.rs`, whose rows are `Owner::Field` (a live field, with a `Ctl`
  saying what control draws it) or `Owner::ActiveCamera`; the toggles are
  registry commands whose palette rows carry a switch, read through
  `command_toggle_state`. The table plus the toggle commands are the app's
  whole display configuration, so a value left out of both is GONE, not
  merely hidden — `every_retired_subnet_setting_is_reachable` is the
  backstop, and `dialog_settings_rows_name_owners_that_exist` round-trips
  every `Field` row because a key no dispatch arm names draws, accepts an
  edit and does nothing.
- **The three point overlays** are `toggle_point_markers` / `_numbers` /
  `_normals`, collected in `rebuild_scene_geometry` off the merged scene
  `Detail` (`render::scene_point_overlays`) rather than by a second walk that
  re-evaluated every flagged node. **Wireframe folded into the existing
  `toggle_wireframe`**, and the survivor draws the TOPOLOGICAL edge list
  (`render::scene_edge_verts`) the per-node flag used, not the triangle soup
  the global one did — shared edges once, quads as quads.
- **Main's buttons** (New/Open/Save/Save As/Set As Default/Exit, Undo/Redo,
  the zoom family, Detach Circular Window) were already registry commands.
  Three settings that were toggles on those nodes and reachable NOWHERE else
  became commands: `toggle_ray_traced_preview`, `toggle_wire_single_color`,
  and `toggle_render_points` — Show Points, which was retired on
  2026-09-29 (see "Display mode" below).
- **The recent-projects list** was the Main node's "Open" dropdown, which
  would have left `recent_files` written and read by nothing. It is rows at
  the head of the palette's Commands list (`RECENT_ROW_PREFIX`), under the
  open project's own path row; picking one opens it.
- **Pane visibility** was the `view` subnet's five toggles riding `fs_root`
  into the file. It is genuinely project state, so it moved to
  `ProjectViewState::visible_panes` beside the collapse list and the
  splitters. `State::PANE_FLAGS` is the one table the save and the load share.
- **The active camera** keeps the viewport menubar's own menu, whose entries
  are the camera NODES — not something a fixed table can hold.

`Project::migrate_meta_settings_node` runs on every load: it takes the meta
node (and the four subnets, which PRE-Session saves parked flat at the root —
hence no early return on the container alone), reads its values onto the live
state, and saves them to `state.kdl`. Per-node children go in
`app::strip_meta_children`, called from `merge_template_defs` because that is
the one function every deserialization runs. Their VALUES are dropped
deliberately: four per-node booleans do not reduce to one global switch, and
inferring one would turn a single node's preference into a setting over the
whole scene.

Gone with them: `session_node()`, `in_settings_dir()` (there is no settings
directory, so Add Node offers every template everywhere), `write_meta_toggle`
and its two wrappers, `refresh_main_node_live_toggles`,
`update_recent_files_layout`, the `utility` / `session` / `meta` node types,
the undeletable-node gate in `delete_node`, and `layout.rs`'s pinning (whose
only pinned nodes were these).

**"World Unit"** (mm / cm / m / in, `State::world_unit`) survives as a
Settings row — what one world unit IS. Geometry never converts; the
declaration feeds two things through the display metric (`cce_ui::units`):
the viewport's bottom-left **scale readout** (`append_scale_readout`:
`1:2.3`, `1 mm = 0.43 mm on screen`, marked when the metric is only assumed)
and the viewport context menu's **View 1:1** (`view_one_to_one`), which moves
the active camera along its eye ray so the pivot plane shows one world unit
at its true length — the default camera by zoom, a camera node by rewriting
its Position, as Frame All does. The projection is a perspective (vertical
FOV 0.9 rad), so 1:1 holds on the pivot plane only; `view_scale_ratio` is the
readout's number.

### App-written settings: `~/.config/cce/cce-designer/state.kdl`

`default_project` in state.kdl points at the project the main window opens on
startup (the `set_as_default` command; absent = the bundled
`default_project.json`). It is a POINTER, never a rewrite of
default_project.json — that file is versioned and is the detached-window sync
channel. Detached windows ignore it: they must keep seeding from the sync
channel.

**A default that cannot be opened is not forgotten.** The launch falls back to
the bundled project and says so on the status line, keeping the pointer. Until
2026-09-23 a path that did not exist was DELETED from the settings, reasoning
that a dead default should not fail on every launch — the trade is the wrong
way round. Failing costs one line of stderr and a fallback that already works;
forgetting costs a setting the user can only restore by reopening the project
and pressing the button again. And a path is absent for reasons that pass — a
cloud-synced folder the daemon has not mounted yet, an external drive, an
autostart that beat the network — so the one launch that raced the filesystem
took the setting with it, silently. (Found exactly that way: a default under
`~/Dropbox` that stopped opening, with the key simply gone from state.kdl.)

**`gpu`** in state.kdl (`integrated` | `discrete`, the dialog's **GPU**
row) picks the device the window's renderer asks Vulkan for. It is read ONCE,
by `app::apply_gpu_preference` in `main` just before `engine::run`, and set
as `CCE_VK_DEVICE` — the variable cce-ui's device selection already reads,
which also steers `gpu.rs`'s compute device — so a change takes effect on the
next launch, and the row's status line says whether the running process is
on it (`gpu_at_launch`). Three rules, all in `gpu_env_for`: an explicit
`CCE_VK_DEVICE` in the environment wins (a per-run override); `integrated`
sets NOTHING, because cce-ui treats any explicit request as licence to lift
the session's `VK_DRIVER_FILES` pin to the Intel ICD, which loads the NVIDIA
driver and wakes the dGPU just to enumerate it; and the thumbnail and export
modes exit before the call, so cce-files' preview cache never wakes it
either. Top-level in `DesignSettings`, beside `default_project`, not in the
render block — that block rides the project file, and which GPU a machine
has is not a property of a scene. Verified under the session's pin: unset
opens the Iris Xe, `discrete` the RTX 4080.

**`playbar_repeat`** (top-level too, since 2026-09-28) is whether playback
wraps at the end of the frame range or stops on the last frame — the
`toggle_playbar_repeat` command, a switch in the palette, unbound. The one
copy is `Playbar::repeat`; with it off a play press on a timeline stopped
at its far end restarts from the near one (`Playbar::begin`, which the
button and the Up/Down chords share).

`DesignSettings` (viewport/graph display state the app rewrites itself:
colors, grid sizes, show flags) persists to `state.kdl` — deliberately NOT
`config.kdl`, which is the user-authored toolkit-config override slot that
cce-ui auto-merges (see `../cce-compositor/WORKSPACE.md`). Legacy `design.kdl` / `design.json`
files migrate on load. Scroll behavior (`scroll_speed`, `inertial_scroll`,
`scroll_friction`) is intentionally absent: it is config-owned
(`input.inertial` in config.kdl) and must not be shadowed by app state.

**Display settings ride the project file too (since 2026-09-24).** Every
save writes `ProjectViewState::display` — a `DisplaySettings`, the viewport
and render blocks of `DesignSettings` without the startup pointer, taken by
`State::display_settings` (which `save_settings` builds from as well) — and
both `load_from_file` paths apply it through `apply_display_settings`, before
the Default Camera view so a camera node's own Square Aspect and pivot
still win. The apply sets every field, regenerates the baked meshes, relays
the two pane-shaped ones (network plate, circular pane), re-checks the
menubar marks, and saves state.kdl, so state.kdl holds the LAST-USED look:
what New and an older save (no block, which changes nothing) open with.
Main window only, as the pane state is: a detached window has no viewport
and reloads the sync channel on every write. A display change dirties the
project (`pane_layout_json` includes the block). `State::new` seeds only
the tree, camera, pan and path from the bundled file, as before, so the
suite does not read the block out of the versioned `default_project.json`.
This reverses the 2026-09-23 position that a display preference should
survive opening someone else's scene — the user's call;
`a_project_keeps_its_display_settings` is the test.

**The path honors `$XDG_CONFIG_HOME`**, resolved through
`cce_ui::config::cce_config_dir()` like every other app in the workspace —
this one hardcoded `$HOME/.config` until 2026-09-23 and was the only holdout.

**And under `cfg(test)` it is a temp directory**, which is the part worth
knowing. `State::new` loads the bundled project, whose meta subnets used to
be copied over the live viewport flags after every parameter change (the
meta node is retired, but the hazard was real and this redirect is what
caught it); so any test that then reached `save_settings` —
`run_command("toggle_network_plate")`, the dialog's toggle rows — wrote the
BUNDLED project's show_grid / show_cube / show_origin over the user's real
state.kdl. `cargo test` reset three of the user's own toggles on every run,
and the run was green either way. `Project::load_recent_files` /
`save_recent_files` are gated the same way, for a variant of the same reason:
cce-ui derives that path from the EXE's basename, so test binaries had left
seven real `~/.config/cce/cce_designer-<hash>/` directories behind. So is the
simnet disk cache (`geometry::sim_cache_path`, since 2026-09-28): a test that
solves a Cache-on simnet writes under a process-scoped temp directory, never
`~/.cache/cce/cce-designer/sim`. That file carries the solve's `prev` beside
its state, so a resume landing exactly on the asked frame draws the interior
view from what the last substep consumed rather than the seed; a file in the
older shape is refused and rewritten.

The redirect is in `DesignSettings::file_path` itself rather than in an
environment variable the test module sets, because a variable leaves the
guarantee resting on every future test remembering to set it BEFORE touching
`State` — and the test that forgets destroys real settings, leaving nothing
behind but toggles that came back wrong. `the_suite_does_not_write_the_users_own_settings`
is the backstop: it spells the real path out itself (`file_path()` being the
thing under test), runs the plate toggle, and asserts both that a settings
file was actually written — or the check is vacuous — and that the real one
did not move.

**The suite's LATTICE is pinned for the same reason**, one layer up:
`configured_grid_geometry` read `style.surface.graph.spacing_x` and friends
straight out of `~/.config/cce/config.kdl`, and the grid tests press at pixel
coordinates derived from `cell_center` and assert which node the press landed
on — so the pitch on the machine decided whether they passed.
`dragging_a_selected_node_carries_the_selection` really did fail at cce-ui's
own defaults (187.5 x 112.5 puts its row 11 at 1237 px in a 900 px test
window, so the press misses the node and the drag never arms); it passed only
because the author's config.kdl set 140 x 70. A fresh clone, a second machine
or CI would all have failed it, reading as a broken drag rather than a
borrowed lattice.

Under `cfg(test)` the four values are fixed at those 140 / 70 / 80 / 40 — the
lattice the grid tests were written against, so pinning them changed no test's
meaning. Deliberately NOT cce-ui's defaults: matching those would mean
rewriting the cell arithmetic of a subtle drag test to fit a coarser grid,
a real change to what it checks for the sake of a number that is arbitrary
either way. What matters is that the number is the suite's own.
`the_suite_runs_on_a_lattice_of_its_own` asserts the constants back — not a
tautology but the thing that fails if the pin is ever unwired to the config
again — and checks the live `State` alongside them, so the pin has to reach
the app and not just the helper. Verified by running the suite under an EMPTY
`$XDG_CONFIG_HOME`, under one setting 999 x 777 with 500 x 400 nodes, and
under the real config: 305 passing, identically, all three.

`graph_grid_snap` is not pinned — it is read inside cce-ui's Graph widget
rather than through this crate, so there is nothing here to intercept; it is
off both by cce-ui default and in practice. Config the suite still reads is
cosmetic in the same way (colors, fonts, plate radii), and no test asserts on
it; the empty-`$XDG_CONFIG_HOME` run is how to check that claim again.

### OpenCL is retired (2026-09-24)

There is no OpenCL in this crate any more: no `opencl` node, no
`kernel_cpu.rs`, no launcher, no `opencl3` dependency, no
`CCE_KERNEL_CPU`. Phase 7 of `shapeshifter.md` is where the decision is
argued; the short form is that the only scripting surface was a C subset
carried by two backends that had to agree, every shipped kernel was serial
(`if (id == 0)`), and the four templates that used them are native nodes
now. Per-element scripting is the `wrangle` node (Rhai, CPU). GPU
parallelism, when a solver needs it, comes back as WGSL compute through
cce-ui's renderer — step 4 of the same phase — not as OpenCL.

**An `opencl` node in an old save is not dropped.** `retired_opencl_node`
passes its input through and reports `<name>: OpenCL nodes are retired;
rewrite the kernel as a wrangle` through the error slot, so the status line
says what happened and the fix is one rewrite. The type stays in
`is_geometry_node_type` for exactly that arm.

**What left with it, for the record.** Mesa's Rusticl ICD closed a file
descriptor it did not own under `clGetPlatformIDs` (caught under `strace
-k` on 2026-09-23), which had the suite failing one run in eight on
whichever template file lost the race, and made `CCE_KERNEL_CPU=1` the only
reliable way to run it. Probing less often did not help; forcing the CPU
backend did not help until it also stopped loading the ICD; what worked was
not loading it. The retirement is the final form of that fix. The
diagnosis is in the git history of this section (commit `8fd0c29`) if the
pattern ever recurs with another driver: a `read` returning EBADF on a file
nothing is wrong with, in a process that has loaded a vendor ICD.

### Parameter kinds and typed values

`src/param.rs` owns `ParamDef`, `ParamKind`, `ParamValue` and `ParamSlot`
(re-exported from `app`). A `type` string names a `ParamKind` —
`ParamKind::parse` reads the head before the first `:` (`slider:-2:2` is a
Slider, `choice:A,B` a Choice; `string`, what an absent type deserializes to,
is Text) — and `ParamDef::kind()` is the one place the string is interpreted.

**A parameter keeps its TEXT and what that text parses to, together.** Both
fields are private; `text()` is the value as written (`"0.50"` stays
`"0.50"`), `slot()` is `Value(ParamValue)`, `Expr` or `Invalid(why)`, and
every setter re-parses, so the two cannot disagree. `set_text` is the one
way a value changes (it keeps the expression flag), `set_value` writes a
typed value, `bake` writes a value and clears the flag (an evaluated
expression, Delete Expression), `set_expr` flips the flag, `set_type` /
`adopt_ui_from` change the kind and re-parse — the template merge goes
through `adopt_ui_from`, so an old save's text wire is a node wire and an
old text Center a float3 from the load on. Tests build parameters with
`ParamDef::new(name, type, text)` and the `with_*` builders.

- **The file format did not move.** Serde goes through `ParamDefRepr`, the
  old struct field for field and in order, so a re-save is byte-identical
  and `sim_solve_key` — a hash of the simnet's JSON — does not restart
  cached simulations. `params_serialize_as_they_always_did` walks every
  template and both bundled projects; the user's own project was checked
  the same way when this landed (128 parameters, identical).
- **A text that does not fit is kept, never coerced.** It loads as
  `Invalid`, readers fall back exactly as they did when every read parsed
  the string (`node_param_f32` on `4.5` in a spinbox is still 4.5), and a
  load says so on the status line (`report_invalid_params`, only over the
  plain success message). What REFUSES one is the two places a person types:
  the params pane (`sync_parameters_to_project`: nothing written, the row
  shows the kept text again, "Not applied — Threshold: 'abc' is not a
  number") and MCP's `set_param` / `add_param`. A value that reads as an
  expression goes through as one and is checked when it evaluates.
- **Readers take the parsed value** (`node_param_f32` / `_vec3` / `_bool`,
  `param_number`) and fall back to the text for a slot that is not the
  kind they want — so a text row holding `12` still reads 12.
- **Expressions evaluate to a typed value.** `eval_param_value` returns
  `Evaluated`: `value_from_expr` converts by the row (a number into a toggle
  is its truth, into a choice the option at that index, into a spinbox its
  whole part), and a result that fits nothing is stored as its text and
  flagged, which is what the old string write-back did.
  `resolve_param_refs` stores either with `set_value` / `bake`.

- **A type naming no kind is refused**, not read as text: `load_fs_tree`
  drops the template with a message (as it does an unparseable one), MCP's
  `add_param` returns an error listing `ParamKind::NAMES`, and
  `every_shipped_template_param_has_a_known_kind` walks the raw files.
  `kind()` itself still falls back to Text so a hand-edited save stays
  editable.
- **`node` is a wire**: every `Input`, Switch's `Input 2`–`4`, Boolean's
  `With`, Collision's `Collider`, Relax's `Rest`, Suture's `Against`, Copy's
  and Distance's `To`, Transfer's `From`. Read them with `node_param_node`
  (trimmed, `None` when unconnected) or resolve them with `param_node`,
  never by hand. By template, not by name: Visualize's `From`/`To` are
  numbers.
- **`attribute` and `group` name a point attribute or a point group on the
  node's input** (since 2026-09-28) — read or written, it is the same kind:
  Visualize's `Attribute` and Normal's are both `attribute`, Relax's `Pin
  Group` and the Group node's `Group Name` both `group`. Any text is a valid
  value; what the kind changes is the params pane, where `add_pick_lists`
  upgrades every row of either kind to a `textpick` (the Houdini chooser,
  a text box with a picker of the input's names) when the input has any to
  offer. Until then the pane knew FOUR rows by (node type, row name) —
  Attribute's `Attribute Name` and `Group`, Group's `Group Name`, Relax's
  `Pin Group` — and the other thirty-odd rows that name one were text boxes
  typed into blind; a template declares it now and needs no entry anywhere.
  What is still `text` is text for a reason: Attribute's `Value` is as wide
  as its `Type` row says (one number or two, three or four) — though the
  pane PRESENTS it as a float3 row, over a deliberately wide
  `VALUE_ROW_RANGE`, when the target is three wide and the text holds three
  numbers (`add_pick_lists`, since 2026-09-28: Create with Type Float3, or
  Modify aimed at Pos, Col or an input Float3; the parameter, the file and
  MCP see text throughout) — Transfer's
  `Attributes` is a comma LIST of names, Simnet's `Start Frame` is empty for
  "the playbar's", Bounds' `Prefix` is a prefix, Curve's `Points` a list of
  positions, Export's `File` a path. The same day Attribute's `From Min` /
  `From Max` / `To Min` / `To Max` became `float` and Neighbour's `Constant`
  a `float3`, the numbers phase 1 missed.
  `template_params_carry_the_kind_they_hold` pins all of it, including the
  rows that stay text.
- **A float3 row can carry a trackball** (since 2026-09-28): cce-ui's
  ball beside the three sliders, which turns the vector's DIRECTION and
  keeps its length, where the sliders set a component at a time. The
  display type is `float3:lo:hi:trackball` (`float3_row`). It is on by
  default where the three numbers are a vector — the Attribute node's
  Value aimed at Pos or a Float3 attribute, the pull node's — and off for
  Col (a colour points nowhere) and for every `float3` parameter (a
  Center, a Size); the row menu's **Show Trackball** / **Hide Trackball**
  chooses either way. The choice is `ParamDef::view` (`trackball`,
  `sliders`, empty for the default), the one piece of UI metadata the
  INSTANCE owns: `adopt_ui_from` fills it from the template only where the
  instance has not chosen, and it is serialized only when set, so a file
  that never used it is byte-identical. The row menu reads such a row as
  `Control: trackball and sliders`, `Type: float3`. With the ball on the
  row writes three decimals. **The ball is seen from the viewport's
  camera** (`State::sync_trackball_view`, called from the stage pass once
  the active camera's pose is resolved): the view matrix's rotation goes
  to the pane as the camera's right, up and toward axes, so the vector on
  the ball lies as the pull arrows do in the viewport beside it, and
  rolling the ball right swings the vector to the right of the SCREEN. The
  numbers stay the scene's. The pane may already be painted when the
  stage pass runs, so a moved view returns true from `stage_frame` for one
  more frame: the ball trails an orbit by a frame, never by more. **A
  detached parameters window follows the camera too**, over the sync
  channel: the main window asks for an autosave when its trackball view
  moves (`sync_trackball_view_from_camera`), the file carries the camera —
  the active camera's name and node, the Default Camera's orbit in
  `default_view` — and the detached window, which has no 3D canvas, works
  the view out from what it reloaded (`active_camera_pose`, split out of
  the stage pass for exactly this). It trails by the autosave's debounce
  and the poll, a few tenths of a second.
- **`float` is a number with no range.** The pane's slider and float3 rows
  hold a FRACTION of their range and clamp to it, so a threshold, a scale
  factor or a manual ramp end cannot be a slider without losing values
  outside it. `param_display` shows `float` and `node` as text rows — cce-ui
  is shared and has neither, and the conversion stays at this app's edge.
- **Toggles read through `node_param_bool`**: `true`/`1`/`on` and
  `false`/`0`/`off` in any case, else the fallback. The sites it replaced
  mixed `== "true"` and `!= "false"`, which disagreed about garbage.
- **Choices are still read as option TEXT** (`eq_ignore_ascii_case`), not
  by index. Matching text survives a template reordering its options; an
  index would silently change meaning. Expressions, which need a number,
  already get the index through `param_number`.

Saved projects need no migration for any of this: `merge_template_defs`
hands every instance its template's type along with the rest of the UI
metadata (`adopt_ui_from`, which re-parses), so an old save's
`"type": "text"` wire loads as `node` (`a_saved_text_wire_loads_as_a_node_wire`).

### Conditional parameter rows

A `ParamDef` may carry `show_when`, a condition over its SIBLINGS' current
values deciding whether the params pane shows it: `Mode == Twist`,
`Mode == Twist|Bend` for any-of, `Mode != Bleed` for unless, ` && ` between
clauses, compared case-insensitively. Empty means always, which is what most
parameters have. `param_visible` evaluates it and `param_display` filters on
it.

It exists because collapsing the Houdini operator set into fewer nodes traded
node count for parameter count — `attribute` reached seventeen parameters, of
which seven apply at once. Phrased the positive way round (unlike Houdini's
`hideWhen`) because a template author is describing when a control APPLIES.

Two rules worth knowing. A condition that does not parse, or names a parameter
the node does not have, HIDES its row: a template bug should be visible, not
silent — and `test_the_shipped_templates_only_name_parameters_they_have` walks
every shipped template to catch exactly that. And hiding a row never touches
its value: write-back resolves rows by display key rather than position, so a
hidden parameter is simply not reported and comes back as it was.

`merge_template_defs` carries `show_when` from the template like the rest of
the UI metadata — the template owns when a control applies, the instance owns
its value.

### Mesh export

`src/export.rs` writes STL (binary and ASCII) and OBJ; `src/export_cli.rs` is
the `--export` mode; the `export` NODE is a pass-through that writes when its
Export button is pressed — never on evaluation, which happens on every redraw
and every frame of a solve.

The formats are not the same picture of a mesh. **OBJ keeps the topology**:
points are written once, faces reference them, a quad stays a quad. **STL keeps
only triangles** — it has no shared points, so everything fans and comes back
welded-by-position at best. Neither carries attributes; the project file and
the sim cache are what preserve a simulation's state.

Coordinates are written as they are, scaled only by the node's Scale.
The World Unit is a DECLARATION, not a conversion (see the Guides node), and
export keeps that promise: geometry modelled at 20 units across writes as 20,
and the slicer is told those are millimetres.

Buttons dispatch through `execute_menu_action` by LABEL, which carries no node
— `run_export` resolves the node from the current selection, which is sound
because the pressed button can only be on the node the pane is showing.

### Mold tooling

`src/mold.rs` is the first GEM operator, ported from the plugin's
`gem_mold_shell`. Its four parameters are that node's — Maximum Thickness,
Minimum Thickness, Remesh Division Size, Thickness Ramp — and the production
notes from the original cast give the numbers that worked (0.75 / 0.6 / 0.9,
linear), which are the template's defaults.

**Thickness varies with curvature**, which is the whole point and the reason
the `volume` node's uniform shell will not do. The plugin does it with an
`im_ramp_scalar` named `curvature_to_thickness`; this does the same three
steps — remesh to the division size, measure curvature per point, map it
through a ramp into the thickness range.

`curvature` is a signed DIMENSIONLESS measure in roughly -1..1: the mean of
`dot(normalize(neighbour - p), n)`. Negative is convex, positive concave. Every
term is a dot product of two unit vectors, so it does not move when the model
is scaled or re-tessellated — which matters because thickness is chosen from
it, and a measure that shifted with the remesh division size would give a shell
whose thickness changed every time you re-tessellated. A true mean curvature in
1/length would do exactly that.

The curvature-to-thickness map is affine over a FIXED -1..1, not normalized
over the range present in the model. Normalizing would make one part's
thickness depend on how curved the rest of it is, so adding a sharp corner
somewhere would thin the whole shell. Concave regions get the maximum: a mould
is weakest where it cups inward, with least material behind it and the most
leverage on it when the cast is pulled.

The inner surface is a DISPLACEMENT along each point's normal, not a field
offset — a signed distance field offsets by a constant and cannot vary per
point. The cost is the usual one: where thickness exceeds the local radius of
curvature the inner surface folds through itself. That is what the
minimum/maximum range is for; it is a range because the geometry constrains it,
not because one number was hard to pick.

There is no ramp PARAMETER type in this app (cce-ui has the widget, nothing
wires it as a node parameter), so the free-form float ramp is ported as the
three-way choice the falloff parameters already use. Linear is the default
because linear is what the cast that worked used.

### Parameter expressions (`ch()` references, Houdini's way)

`src/expr.rs` is the expression language and `geometry.rs`'s `TreeScope`
is what binds it to the node tree. **A parameter holds a value or an
expression, and `ParamDef::expr` says which** — a flag, not a guess about
the text, because a kernel's Code contains `chf(`, a node name is an
identifier and `0.5` parses as an expression too. Houdini makes the same
choice (a parm has a channel or it does not). An expression parameter is
evaluated every time its node is: `resolve_param_refs(root, node, frame,
error)` hands back a clone whose `expr` params are VALUES, at the top of
`generate_single_node_geometry_with_errors`, the scene walk's `visit`, and
the kernel path's parent read.

**Paths are Houdini's.** Relative to the node holding the expression: a bare
name is the node's OWN parameter, `..` its parent, `../sphere1/Radius` a
sibling's, a leading `/` the root. `.x` / `.y` / `.z` reads a float3
component. `ch()` / `chf()` read a number (a toggle 1 or 0, a choice its
option INDEX — `chi("../Method")` is what lets a subnet's dropdown drive a
child switch's Index), `chi()` truncates, `chb()` is 1 or 0, `chs()` the
string (a choice's option text). The rest is `+ - * / % ^`, comparisons,
`&& || !`, `$F` (the evaluation's frame), strings with `+`, and a fixed
function set (`if(c, a, b)`, `clamp`, `fit`, `lerp`, `min`/`max`, `rand(seed)`,
the usual math). No ternary — `:` separates a float3's components, which
are three expressions each (`chf("../a/Size.x"):0:0`). An expression that
reads an expression follows the chain; a circle is an error on the node,
never a stack overflow. The written-back value is formatted for the
TARGET row (`format_for_param`): a number into a toggle is `true`/`false`,
into a choice its option name, into a spinbox an integer.

**Until 2026-09-24 a bare `ch("Name")` meant the PARENT's parameter** (the
whole value had to be one reference, nothing else). `Project::format` is
the version that tells the two apart: 0 (absent) loads through
`migrate_param_refs`, which turns each old reference into an expression
with `../` added to a bare name, and saves as 1 — beside
`sanitize_node_names` on every load path, and never twice, since a bare
name in a format-1 file is the node's own parameter. Templates go through
`infer_template_exprs` instead: a default that READS as a reference is one
(`embryo.json` says `chf("../Radius")` now). The same inference applies to a
value typed into a plain row or scripted through `set_param`: a reference
becomes an expression; bare arithmetic does not, and is asked for through
the row menu.

**The params pane's right-click menu** (`param_row_at` → `open_param_context_menu`,
a fifth `context_menu` consumer with the `*_menu_actions` +
`handle_*_menu_click` contract, and `run_param_action` as the one entry the
menu and the tests share) is Houdini's: **Copy Parameter**, **Paste
Relative Reference** (`relative_ref_path`: `../sphere1`), **Paste Absolute
Reference** (`/sphere1`), and **Edit Expression** / **Delete Expression** —
the latter bakes the CURRENT value back as a value, as Delete Channels
does. `copied_param` holds a node ID, not a path, so a rename between copy
and paste still pastes the right path. **Header rows read the
parameter out** (since 2026-09-28, `param_menu_rows`): `Name:` is the
parameter's name, what a `ch()` path spells, with `Label:` under it only
when the template gives one (the pane shows the name otherwise, and a
Label row repeating it would claim there is one); `Control:` is the
control the pane DRAWS for the row (slider, spinbox, dropdown, toggle,
text box, text box with picker, code editor, button) and `Type:` the type
of value that control SETS, in a programmer's terms (float, float3,
integer, boolean, enum, string, and node / attribute / group for a text
that names one) — `control_and_type`, read off the row as the pane shows
it, since the Attribute node's Value is a text parameter presented as
sliders over a float3 and an expression is a text box whatever its kind.
Until later the same day Control was the kind's name, Type the raw type
string and a third `Value:` row the type of the text held, which read
`text` / `text` / `string` over what was plainly a slider setting a
vector; the Value row is gone, the Type row being the value's type, and
the raw string's content is the range and options rows. `Expression:` is
the row's expression FLAG as `true` / `false` (the bit Edit Expression
sets and Delete Expression clears), `Invalid:` the reason a kept text does
not fit its kind, shown only then, `Default:`
the template's value as written there
(`State::template_default`, which takes a subnet template's override for
a child inside an instance — the Embryo's `sphere1` defaults its Radius to
`chf("../Radius")` — and is absent for a parameter no template names),
then for a slider, float3 or spinbox `Min:` / `Max:` / `Step:` as the
template DECLARES them (`ParamDef::declared_range`, an inline
`slider:-2:2` included, `none` where it says nothing) followed by
`Range: lo..hi` with its step as the pane APPLIES it (`ParamDef::range`,
the numbers `param_display` builds the pane's row from — the declared
ends, else the pane's defaults — or the presented row's own range for a
parameter that has none, the Value row's `VALUE_ROW_RANGE`),
`Options: a, b, c` for a choice, and `Shown when:`
with the row's `show_when` condition when it has one. They are the context menu's header rows —
dimmed, never hovered — and `ParamMenuAction::Info` runs nothing. The paste writes `chs()` when the
target row holds text or a choice and `ch()` otherwise, by the TARGET,
because that is what the value has to fit. Expression rows draw with a
green tint (`render.rs`, PARAM_IDX arm) and as text in the pane
(`param_display`), since a slider cannot hold one.

**A rename carries every reference to the node** (`rename_node_in_tree`):
expression paths that pass through it are rewritten textually
(`expr::rewrite_paths`, so spacing survives), resolved from where each
stands BEFORE the name changes since a path is names; sibling wires whose
value is the old name follow, as the load-time sanitizer rewrites them;
and the active camera. A same-named node elsewhere is not this one.

### Bypass

A node can be BYPASSED (since 2026-09-29): it stays in the graph, wired as
it was, and does nothing. `FsNode::bypassed`, and `geometry::is_bypassed` is
the one reading of it.

- **What reads a bypassed node gets what the node reads**: its `Input`,
  untouched. A node with no input — a generator — gives nothing, which is
  what a generator switched off should give. A bypassed subnet or simnet
  passes its Input and its children are not run.
- **It is decided ahead of the node's own parameters.** The check sits at
  the top of `generate_single_node_geometry_with_errors`, before
  `resolve_param_refs`, so an expression that would fail on the node is
  not evaluated and not reported: bypassing is how a broken node is taken
  out of a chain while it is fixed.
- **In three places, because there are three dispatches**: that function,
  the scene walk's `visit` (which hands nodes to their resolvers itself —
  a shown, bypassed node draws what it passes, is still counted for the
  nodes placed by index, and is not gone into), and `page::resolve_page`
  for the 2D context.
- **`input`, `output` and `camera` ignore it.** The first two are a
  subnet's plumbing, and a bypassed `input` would be a chain that reads
  nothing.
- **The flag is written only when it is set** (`skip_serializing_if`), so
  a file that never bypassed anything is byte for byte the file it was and
  `sim_solve_key`, a hash of the simnet's JSON, restarts a simulation when
  a node in its chain is bypassed and not otherwise.

`State::set_bypassed(slots, bool)` is the one writer, and three things
call it: the **`bypass_node`** command (`b`, the network's, acting on the
selection, the whole of which follows the FIRST node's flag as `e` does
for the geometry flag); the node's right-click menu (**Bypass** / **Stop
Bypassing**); and MCP's `toggle_bypass`. The id is not `toggle_bypass`
because the `toggle_*` family is the settings' switches, which
`dialog_toggle_rows_cover_every_toggle_command` holds to a table this has
no place in.

A bypassed node wears amber (`render::BYPASS_TINT`): its roll tinted
through the bevel's own tint channel, and a flat bar down its left side,
drawn after the bodies with the geometry toggles. The bar is what still
says so while the node is selected and its roll is the selection's colour.
The node is found by `GraphController::node_at` at the body's centre, so
the second network editor marks its own level's. Nothing in cce-ui
changed.

`a_bypassed_node_passes_its_input_through` covers the geometry resolver
and the scene walk, `a_bypassed_page_node_passes_its_sheet_through` the
page resolver — with each node of a sheet, grid, border and export chain
bypassed in turn — and `bypass_is_one_flag_however_it_is_asked_for` the
three ways of asking.

### Sibling-first inputs and the Switch node

Two pieces added on 2026-09-21 so a node can be BUILT FROM other nodes
the way a Houdini HDA is — the Embryo is the first to be recomposed that
way — both in `src/geometry.rs`:

- **`find_input_node(root, target, name)` looks for a SIBLING first, then
  anywhere.** Every resolver used to search the whole tree from the top, so
  inside the second instance of a subnet a child wired to "input1" found the
  first instance's; the opencl and output resolvers had each grown a
  sibling-first lookup of their own to dodge exactly that. Every wire goes
  through it now, most as `param_node(root, target, "Input")` (see
  "Parameter kinds"). That was claimed on 2026-09-21 and was not quite
  true until 2026-09-26: Collision's `Collider`, Relax's `Rest`, the page
  chain's `Input` and the params pane's group/attribute pickers still
  searched the whole tree by name, so in a second copy of a subnet they
  found the first copy's node (`a_rest_wire_resolves_to_its_own_sibling`).
  Which rows GET a picker is the parameter's kind — `attribute` / `group`,
  see "Parameter kinds" — not a table of row names.
- **`switch`** passes one of `Input`, `Input 2` … `Input 4` by `Index`,
  clamped; an empty slot passes nothing. Only `Input` draws a wire, the
  limit every second operand has (Boolean's With, Copy's target).

### Sphere, Box, Plane and Extrude are native (2026-09-24)

`src/shapes.rs` holds the four shapes that were kernel subnets — Phase 7
step 3 of `shapeshifter.md`. Each was `input → opencl → output` with a
kernel that ran under `if (id == 0)`: one work item doing loops, then a
weld by position on the way back that threw away every shared point the
loop had known. Native, each builds welded points and real primitives —
a quad stays a quad — costs no JIT compile and needs no OpenCL at all.
The parameter surfaces are the templates' own, so a saved instance keeps
its values. The templates are plain native nodes now (`"type": "sphere"`
and so on, no children); the Embryo is the one subnet template left, and
its `sphere1` child resolves to the native Sphere with the template's
whole surface under the Embryo's overrides.

**The Sphere's Method** — `UV`, `Icosphere`, `Cube` — survives as it was:
UV is Rows x Columns through `sphere_detail`; Icosphere splits each of the
icosahedron's 20 faces into Frequency^2 triangles by integer barycentric
weights; Cube lays a Resolution x Resolution grid on each face and pushes
it out through the spherified-cube map, and builds QUADS where the kernel
fanned them. Welded counts are `2 + (rows - 1) * cols`, `10 f^2 + 2` and
`6 r^2 + 2`, which `sphere_method_builds_a_uv_ico_or_cube_sphere` asserts
along with closedness. Welding is by a QUANTIZED position key (1e-5)
rather than by trusting bit-identical arithmetic across faces: the kernel
summed weights in one fixed expression so shared corners landed on the
same bits, then welded at 1e-4 anyway; a quantized key is the same
guarantee stated once. Colour is the kernel's — the SIGNED normal folded
into 0..1, world-anchored — with Color on, `DEFAULT_COLOR` off.

**A bare `sphere` node with no Center parameters is placed by index**
(`index_center`), the way Line and Points still are: that is the tests'
hand-built `ref_node("sphere", [Radius])`, and every node that came through
a template or a load carries Center X/Y/Z and sits where they say.

**Box** is eight corners and six quads about a float3 Center (new; the
kernel hard-coded (0, 0.55, 0)), normals on the VERTICES like `box_detail`;
Wireframe draws the twelve edges as bars and the corners as small cubes,
as the kernel did. Its unused `Input` is gone. **Plane** is the kernel's
sheet with its colour gradient; Grid is the same sheet with a float3 Center
and no gradient, two nodes for history's sake.

**Extrude extrudes AS A WHOLE**, which is the one semantic change: every
point moves along its point normal, the input's primitives become the
top, one quad wall rises from each BOUNDARY edge, and Keep Base keeps the
originals wound the other way — a sheet becomes a closed slab, a closed
surface a two-skinned shell. The kernel extruded every triangle on its
own and welded the prisms back together, which put a wall along every
interior edge. Point attributes and groups ride to the top copies; the
kernel's 15% darker walls were a per-corner colour a soup could hold and
shared points cannot, and are gone.

**Saved kernel subnets migrate on load.** `nativize_kernel_subnets` in
`merge_template_defs` turns a `node` whose children include an `opencl`
child and whose base name is one of the four into the native node: id,
name, position, flag and values stay, the children go, and a parameter the
native template lacks goes with them. Only when the `opencl` child is
actually there, so a subnet someone built by hand and called "sphere2"
keeps what is inside it. The bundled `default_project.json` and
`project.json` were converted in place, and
`test_loader_merges_new_template_params` is the migration's test. The
`opencl` node and both kernel backends were retired the same day (above).

### The Embryo node is a template of nodes

`nodes/embryo.json` is hou-control's `developer_embryo`, the Developer
family's first Pre-Simulation operator — "the seed geometry a simulation
starts from" — as a SUBNET of ten ordinary nodes wired the way the HDA's
network is, its controls reaching the children through parameter references
(above). Dive in and the pipeline is there to read, break and reuse: `input1`
and a `sphere1` (Radius `chf("../Radius")`, Rows and Columns
`chi("../Base Resolution")`) behind `source1`, a `switch` whose Index is
`chi("../Source")`; `scatter1` in Surface mode reading the Scatter folder's
controls, `hull1` behind it, and `method1`, a switch on `chi("../Method")`
between the source and the hull; then `relax1` in Repel mode, `subdivide1`,
`normal1`, `output1`. The defaults are the HDA's, and
`embryo_template_builds_a_sphere_a_hull_or_the_input` drives the template
end to end.

It was a native node for one day (2026-09-21, `src/embryo.rs`, a pipeline in
Rust), which is the wrong shape for this app: CLAUDE.md refuses `gem_graph`
for the same reason, and a node you cannot dive into cannot be learned from.
Recomposing it needed four reusable pieces, all of which outlive it:
parameter references (now expressions, their own section above) and the
`switch` node, the
`hull` node (`src/hull.rs` — the incremental convex hull; points that span
no volume pass through), and two modes on existing nodes (`src/scatter.rs`):
**Scatter's Surface mode** (points ON the surface by area, seeded, optionally
pushed apart across it with a radius derived from the area per point — the
Scatter SOP with Relax Points) beside its original Volume mode, and
**Relax's Repel mode** (spheres of Radius pushed apart, sliding in the
tangent plane unless In 3D Space; zero iterations is off) beside its
original Springs mode. A native `embryo` in an older save is recomposed on
load (`recompose_native_embryo` in `merge_template_defs`): id, name,
position, flag and values carry over, the template's children arrive fresh.

Two deliberate differences from the HDA. **Subdivide does not smooth**: it
is this app's `remesh::subdivide` (four triangles per triangle, points
unmoved), where the HDA runs Catmull-Clark — same parameter, one operation
rather than two under one name. **The second input is the first**: the HDA
read its Source from input 2, and this app's nodes name one Input.

**Exactly one child of the template draws, `normal1`**, the last real
node. A subnet viewed from
OUTSIDE shows its internals by their own flags (output children draw only
at the displayed level), so with every chain node visible the hull drew
five times over, each draw re-evaluating the pipeline: 2.4 s per edit on a
release build, 0.1 s with one. Template child specs therefore carry
`geometry_visible` through `load_fs_tree` (absent means on, as before).

Nesting a subnet template inside a template (the Embryo's `sphere1` is the
Sphere template) is what made `load_fs_tree`'s child resolution recursive:
a base that is itself a subnet brings raw children of its own, and those
resolve the same way, or the nested sphere's kernel node arrived with only
the params its override named. Depth-bounded, so a template that contained
itself would fail rather than recurse forever.

### The wrangle node

`src/wrangle.rs` is a script run once per element, on Rhai — Phase 7 step 1
of `shapeshifter.md`, and the app's scripting surface for per-element work
where the retired `opencl` node used to be the only one. The engine is a dependency;
what the module owns is the BINDING to the `Detail`, and it is VEX-shaped on
purpose so `@P.y += sin(@P.x) * 0.1;` reads as it does there.

`@name` is sugar. Rhai has no `@` token, so `desugar` rewrites `@name` into
an index on an element marker (`__at["name"]`) outside strings and
comments, and everything after it — `.x`, `+=`, `[0]` — is Rhai's own syntax
on the value that came back. The indexers reach the geometry through a
shared context; the marker is a VARIABLE in the scope, not a constant,
because Rhai refuses to assign through an indexer on a constant and `@P = …`
is exactly that (the first cut used `push_constant` and every write failed
with "Cannot assign to indexer of constant"). Naming an attribute creates it,
typed by the first value written — a float, an int (a bool is an int), a
`vec3`, an array of two or four — and a write to an existing attribute
converts to ITS type, so `@mass = 2` into a float attribute is `2.0`. A
float2 reads back as a `vec3` with z = 0, a float4 as an array. `@P`, `@Cd`,
`@N` (computed on read when absent), `@id`, `@ptnum` / `@primnum`, `@numpt` /
`@numprim` and `@Frame` are intrinsics; on the Primitives class `@P` is the
centroid and read-only, and on Detail `@name` is a detail attribute.

**`ch("path")` is resolved BEFORE the run, not called during it.**
`channel_refs` scans the script for the paths it names as string literals,
and the evaluator in `geometry.rs` resolves each through the expression
`TreeScope` — the one scope, so a parameter that is itself an expression is
evaluated first and the script sees its value; that is the seam Phase 7's
step 2 names, and neither language knows the other exists. Two things follow:
`ch` costs a map lookup per element rather than a tree walk, and a path built
at runtime is an error that says why. `chs` reads text, `chv` a float3, `chi`
truncates.

`neighbours(pt)`, `prims(pt)`, `points(prim)` read the derived topology and
`nearest(pos, r)` the point grid — built once at the first call from the
positions as they then stand, and keyed by radius. `point(name, i)` /
`setpoint`, `prim` / `setprim`, `detail` / `setdetail`, `ingroup` /
`setgroup` reach elements other than the current one. `addpoint`, `addprim`
and `removepoint` are DEFERRED and applied after the run, so a script
iterating points sees a stable count; `addpoint` returns the index the point
will have, which is what makes `addprim([a, b, c])` in Detail class a way to
build geometry from no input at all — a wrangle with nothing wired still runs.

Ints and floats mix (`@P.y * 2` works), which Rhai does not do on its own;
the mixed arithmetic and comparison operators are registered by hand, as are
`vec3`'s. Two budgets: `OPS_PER_ELEMENT` operations per element, which is
the retired `kernel_cpu`'s step budget as a setting rather than a hand-rolled counter,
and `RUN_BUDGET` seconds of wall clock for the whole run, checked in
`on_progress` every few thousand operations. Any failure — syntax, a runtime
error on an element, a budget — fails the WHOLE run, named by node and
element (`wrangle1: point 4: …`), and the input passes through untouched: a
half-wrangled geometry is not a result. Compiled scripts cache by desugared
source in a thread-local, as the retired launcher cached kernels.

CPU only, deliberately: an interpreter is an order of magnitude or more
below native Rust, which is fine for tens of thousands of elements per edit
and wrong for a solver at a million per frame. That is Phase 7's step 4
(WGSL compute through the renderer), not a reason to grow this.

**The Code row applies on ctrl+enter, Escape or leaving the row — never per
keystroke.** cce-ui's `ParametersBg` code editor (line numbers, selection,
clipboard, tab indenting, auto-indent, undo) keeps edits in its buffer
until one of those, because this node evaluates on every value change and
a half-typed line would fail on every keystroke — the border is amber
while edits are pending. **A script error's line is flagged in the row**:
`code_error_line_for_pane` in `render.rs` reads the `(line N` out of the
evaluation error when the node it names is the one the pane shows, and
hands it to `set_code_error_line`; Rhai's line numbers survive `desugar`
because the `@` rewrite never adds or removes a line. Cleared on the next
evaluation that says nothing about that node.

### GPU compute: the springs solve is the first operator (Phase 7 step 4)

`src/gpu.rs` keeps one `cce_ui::vk::ComputeDevice` per thread, opened on
first use and kept, so the pipeline cache and the buffers survive from one
edit to the next; a device costs tens of milliseconds to open and a kernel
a few to compile, and an operator that paid both per evaluation would lose
to the CPU every time. `CCE_COMPUTE` decides: unset or `auto` takes the GPU
when there is one and the operator judges the input big enough; `cpu`
never opens a device; `gpu` insists, and an operator that cannot get one
says so through the node-error slot rather than silently taking the CPU
path. **Under `cfg(test)` auto means CPU**, so the suite is the same on
every machine and the GPU is exercised only by the tests that ask for it
by name — the cross-checks. The suite never SETS the variable: libtest
runs tests in parallel and one that did would race every other test
reading it (`gpu::parse` is the pure function the choice test covers).

`src/springs.rs` is the pattern every later operator follows: one
algorithm, one data layout, two backends held to each other by a
cross-check (`springs_gpu_matches_cpu`, agreement to 1e-4 over 1.5k
points; it skips with a note where there is no Vulkan). The layout is the
GPU's — positions as a flat `xyz` array because a `vec3<f32>` in a WGSL
storage array pads to 16 bytes, the rest topology as CSR with the rest
length on each incident entry, pins as one `u32` per point — and the CPU
walks the same arrays in the same order. `solve` chooses the backend; a
GPU failure in auto mode falls back to the CPU with one stderr note.

**Relax's Springs mode is a JACOBI solve now.** Until 2026-09-24 it was
Gauss–Seidel over the edge list in sequence, every correction visible to
the next edge, which no per-point kernel can reproduce; rather than let a
GPU Jacobi and a CPU Gauss–Seidel drift apart, both run Jacobi: each point
gathers the corrections of its incident edges from the pass's starting
positions — half the error toward a free neighbour, all of it toward a
pinned one — averages them, and moves once. It converges roughly half as
fast per iteration, which Iterations already controls; the pinned-pull
test that defines the node's behaviour passes unchanged.

**The whole solve is ONE submission** (`run_passes_over` with a ping-pong
pair): the topology goes up once, the passes are chained by memory
barriers with the positions alternating between two device buffers, and
the result comes back once. The first cut submitted a pass at a time and
LOST to the CPU at every size measured, 134k points included — a
submission's round trip is about half a millisecond on an integrated GPU
whatever the dispatch inside it, and sixteen of them buried a solve that
takes microseconds. `springs_timing` (ignored; run in release with
`--ignored --nocapture`) is the measurement, on an Intel Iris Xe, sixteen
passes: 1.5k points cpu 0.25 ms / gpu 1.5 ms; 15k cpu 2.6 ms / gpu 3.7 ms;
135k cpu 25 ms / gpu 14 ms, plus ~15 ms of pipeline compile on a device's
first run. `GPU_MIN_POINTS` (32k) is the auto threshold that follows: the
GPU is a win for large meshes and a loss for the ones most projects have,
which is the honest state of step 4 and the reason auto does not simply
mean GPU.

**Collision is the second operator (`src/collide.rs`), and the one the
GPU is made for.** The node's test has always been a brute-force loop —
every query against every collider triangle, the Voronoi-region distance
for Proximity and a Möller–Trumbore parity cast for Inside — so the work
is queries x triangles, per-point, one dispatch, no passes to chain. The
resolver now runs the test as ONE batch over every element the type asks
about (points, or primitive centroids), where it used to hand
`select_elements` a closure that asked one point at a time; that batch is
what can go to the GPU whole. Same algorithm step for step on both sides,
held by `collision_gpu_matches_cpu` (zero disagreements over 6k queries x
1.7k triangles in both modes; a knife-edge query at the threshold may
round either way and is tolerated only there). `collision_timing` in
release, Proximity: 3.6M pairs cpu 20 ms / gpu 2.2 ms; 15M pairs cpu 76
ms / gpu 6.6 ms; 242M pairs cpu 1150 ms / gpu 52 ms. `GPU_MIN_WORK`
(250k pairs) is the auto threshold.

**Two per-point operators are deliberately NOT on the GPU, and the
measurements above say why.** Neighbour's Diffuse and Concentrate are a
single gather per evaluation — one pass, then the rest of the graph runs
on the CPU before the next frame's pass — so there is nothing to chain
into one submission, and a single pass costs ~0.5 ms of round trip against
a CPU gather that takes less than that on any mesh a project has. Relax's
Repel rebuilds a spatial grid every pass, which is the part that does not
fit a chained submission; a GPU-side grid is a project of its own, and a
brute-force O(n^2) pass that the CPU twin would then have to match is a
regression for every CPU user. Both stay native until a workload asks.

### The Detangle solve

`geometry::apply_detangle` is the node and says what it does; `src/detangle.rs`
is how it is run. The algorithm did not change on 2026-09-29, what it
costs did: measured on a 162-point simnet of pull, relax and detangle, the
node was nine tenths of the solve (0.35 ms a step against 0.03 for the
rest), and four things it paid for every step were things a step does not
need.

- **The topology is built once.** The edge list and each point's excluded
  rings are connectivity, which this chain never changes — but every step
  arrives as a fresh `Detail`, whose derived topology is deliberately not
  cloned. They are kept per thread by a hash of the primitives (`Topo`,
  the last four), so a solve of a hundred steps builds them on the first.
- **A pass that separates nothing ends the solve.** The passes gather
  against the positions at their start, so the next would find the same.
  A surface that touches itself nowhere is one pass, whatever Iterations
  says.
- **The grid is reused between passes** while no point has drifted more
  than half a cell from where it was filed (`DRIFT_CELLS`); the search
  reaches a drift further than the thickness, so a moved point is still
  found. It is flat — one array sorted by cell — where `spatial::PointGrid`
  is a vector per cell and allocated a scratch list per query.
- **A point outside the Group is not searched for.** Its push was worked
  out and thrown away.

**The results are the first version's bit for bit** — the same pairs,
summed in the same order (candidates ascending, as `PointGrid` sorted
them) — which is what makes these optimizations and not changes. The
first version is kept as `apply_detangle_reference` under `cfg(test)`,
and `the_detangle_solve_matches_its_reference` runs both step after step
over meshes that tangle and ones that do not;
`the_detangle_solve_skips_what_it_does_not_need` reads `detangle::Work`
for each saving. On the project it was measured on, detangle's share of a
solve to frame 30 went from 10 ms to 2, to frame 120 from 46 to 15, and
to frame 240 from 113 to 71: once the whole surface is within a
thickness of itself every pass runs and the pairs themselves are the
work, and no bookkeeping saves that.

**The Surface method and the measure (2026-09-29).** Everything above is
the node's `Points` method, which is what a node without a `Method` row
runs and what the template defaults to, so a save from before solves as it
did. `Method: Surface` (`detangle::solve_surface`) tests each point against
the TRIANGLES near it: a point over the middle of a triangle is near no
corner of it, so where triangles are larger than the thickness the point
test sees nothing at all. A contact is resolved along the line from the
closest point on the triangle to the point (the triangle's normal where the
point lies on it), and the move is SHARED — the point one way, the corners
the other by how much of the closest point each is
(`spatial::closest_weights_on_triangle`), with what is outside the Group
taking none and the rest all of it, so a contact with a fixed triangle is
resolved whole where Points resolves half. What a point receives from
several contacts is their average weighted by depth, not their sum: a point
over a shared edge touches both triangles and must move once. A triangle
with a corner inside the point's excluded rings is not a contact.

**Inside a simnet a point has a SIDE** (`detangle::apply_from`, the same
day). `resolve_detangle_geometry_with_errors` hands the solve the state the
substep consumed — the nearest simnet above the node that has pushed one,
so a detangle in a subnet inside a simnet gets it too — and a `before` that
is not this mesh (another point count, other primitives) is not used.
Outside a simnet there is none, and Surface is the distance test alone.
Three things read it:

- **The passes put back what went through.** `went_through` asks in the
  TRIANGLE'S terms, since both move: the point's height over it and the
  place of its foot in it, then and now, a straight line between. A sign
  change with the foot inside is a passage, and the contact is resolved
  along the triangle's normal to a thickness clear on the side the point
  came from. A point with such a contact takes no other that pass — the
  triangles beside the one it went through see it near, on the wrong side,
  and would push it on. The passes take NO margin on "inside": a tenth of
  one pushed points off triangles they had gone around and left more
  crossed than no memory at all (642 points, Thickness 0.5: 40 beyond the
  rings against 0).
- **The hold.** Whatever is still through a triangle when the passes are
  done goes back to where the step began, its triangle's corners with it
  (`HOLD_ROUNDS` looks, a margin of `HOLD_MARGIN`, since holding a point
  that did not quite go through costs it a step's movement and nothing
  else). The memory is one step long — a point left through is, to the
  next step, a point that began there — so this is what keeps a miss from
  becoming permanent.
- **Step Limit** (a row, shown for Surface, in thicknesses, 0 = off and
  the default) cuts each movable point's move since the step began to that
  length before anything is resolved. Off by default because it changes
  how far a pull pulls, and because the measurements did not earn it a
  default: it is for use WITH Substeps.

`the_surface_method_puts_back_what_went_through` carries a patch through a
fixed sheet in one step, by less than a thickness and by several, and
around its edge; `the_step_limit_holds_a_step_to_a_length` and
`a_detangle_in_a_simnet_knows_where_the_step_began` (in `geometry.rs`'s
tests, where the feedback stack can be reached) are the other two.

**Edge Contact** (a toggle, shown for Surface, on in the template; a node
without the row is one from before it and reads off). Two edges can pass
through each other with no point of either going through any triangle:
on the sphere test at Thickness 1 the solve above put nothing back and
held nothing, and still left 48 crossings beyond the rings at half an edge
a step and 98 at four fifths. With it on, every side of every triangle
(`Topo::sides` — the mesh's edges and the diagonals a fan cuts across a
quad) is tested against the sides near it that share no neighbourhood
with it, a pair met once from the earlier of the two. Where the two are
nearest at a place INSIDE both (`nearest_on_segments`; an end is a point,
and the point test has it) they are parted along the line between those
places, the four ends sharing the move by how near each is. Told where the
step began, two edges that went through each other (`edges_went_through`,
`went_through`'s question asked of two lines, and refused where the two
have swung past running the same way, which turns their own direction
over) are put back, and held if the passes leave them through. A contact
is one shape for both tests — four points, a share each, a direction — so
the averaging and the group rule are written once.

**Which edges are tested is a bound, not a guess.** Two edges nearer than
`close` somewhere along them have an end within that and half the edge's
length of the other edge, which is a side of a triangle; so the point loop
looks that far (`close + half[p]`, half the longest side at the point) for
a triangle with at least two corners outside the point's rings, and an
edge with neither end near one is skipped. The first cut flagged a point
only when a triangle was within a THICKNESS of it, which cost nothing and
kept every number on the sphere test — and skipped the one case only an
edge test can see, two edges meeting at right angles with every point far
from the other triangle. `edge_contact_parts_edges_no_point_test_can_see`
is that case, near and carried through.

What it costs, 2562 points, release: nothing is searched on a round
sphere at Thickness 0.5 (5.0 ms against 2.7, the wider look), a quarter of
the edges at Thickness 1 (11 ms against 5); on the sphere test, where most
of the mesh is in contact, 129 ms a step against 24. What it buys there:
NO crossing beyond the rings in any of the twelve runs (three sizes, two
paces, two thicknesses), where the side alone left up to 140. What it does
not: folds inside the excluded rings, which the measure still counts and
which in the thin, fast runs were MORE with it on (545 against 358 at
2562 points, Thickness 0.5, four fifths of an edge a step).

**Fold Contact** (a toggle, shown for Surface, on in the template, inside a
simnet only; a node without the row reads off). The rings are excluded
from contact because a neighbour is nearer than a thickness by
construction, and no distance says whether it is too near — so a surface
folding through its own neighbourhood was seen by nothing, and on the
sphere test everything the other rows left was that. But going THROUGH is
not a distance. `detangle::folded` asks the through question of every pair
inside each other's rings that shares no point: a point and the triangles
at the points of its rings, a side and the sides at the points of its
ends' rings (sides only with Edge Contact on). It walks the MESH
(`Topo::tris_at` / `sides_at`), not the grid: what is in a point's rings is
in them however far apart the fold has left them. It runs once when the
solve begins, and what it finds is a contact in every pass — put back as
far over its neighbour as it BEGAN (`height.min(thickness)`), not out to a
thickness, which a neighbour never was — and again in each look of the
hold, which returns what is still folded to where the step began.

**When something went through is searched for, not read off**
(`crossing_time`). The first `went_through` took the moment and the place
from a straight line between the two ends' heights and weights, which is
right for a small step and wrong for a long one: a point carried across
several triangles was said to have gone around the one it went through,
and two edges were put back on the wrong side of each other. Both tests
now take the volume the four points span, which changes sign when they
are in one plane, find that moment by halving, and ask where the foot (or
the lines' meeting) was THEN. `fold_contact_puts_back_what_went_through_its_own_neighbourhood`
is the fixture that showed it.

With all three on the sphere test ends with NO crossing of any kind, at
any step, in all twelve runs (the `all` row), where the side and the edges
together left up to 604. What that costs: about seven times the Surface
method told the side alone where most of the mesh is in contact (234 ms a
step against 32 at 2562 points, 12 against 2 at 162), most of it the
edges'. And it is bought by HOLDING: some 200 points a step at 2562 are
put back where the step began, a tenth of the mesh, so a fold that is
being forced stops moving there rather than folding. That is the
guarantee working, and it will read as the surface sticking.

**How the Surface method is run (2026-09-29, later).** The costs quoted
above are from before this and are kept as the record of what each piece
cost when it landed; what it costs now is `detangle_timing` (ignored;
release, `--ignored --nocapture`), the sphere test with every row on: 6 ms
a step at 162 points, 11 at 642, 27 at 2562, where it was 9, 44 and 186 —
and Surface alone 4.6 ms at 2562 where it was 18. The test prints the sum
of every position at every step, and that sum did not move through any of
the three changes (-55113.128580 at 2562 points): they are how the solve
is run and not what it does. Timed split by phase first, which is what
said the cost was not the contacts and not only the edges: every pass, and
every look of the hold, was repeating one spatial search.

- **What is near what is found once** (`detangle::Near`): the pairs a pass
  looks at — a point and a triangle, two sides — are listed when the solve
  begins, with half a thickness to spare (`SLACK`), and kept until a point
  has moved half of that. The passes and the hold walk the list. On the
  sphere test that is 1.0 to 1.3 searches a step where there were five or
  more.
- **A pair is listed by its DISTANCE, not its box.** On one sheet the
  sides a few edges off are near enough for their boxes and further than
  any thickness; listing by box put 197k pairs of sides on the list at
  2562 points, by distance 79k. What may have gone through since the step
  began is within twice what anything has travelled of what it went
  through, so that, or the thickness, is how far a pair may be.
- **The hold looks again only at what it put back** (`stirred`): a pair
  none of whose points moved since it was last looked at is as it was.
  The first look is at everything.
- **The work is cut into pieces and run on every core** (`in_pieces`):
  the search, the fold sweep, the pairs of a pass, the pairs of a look.
  Each piece makes its own list and the lists are put end to end in order,
  so the result is what one thread would have made, contact for contact —
  which is why the sum holds. `std::thread::scope`, not a pool: the crate
  has none, a thread costs tens of microseconds to start, and each call
  names the least a piece may be so that small meshes stay on one thread.
  Most of the gain is this, and it is the machine's: on the twenty threads
  it was measured on, 149 ms became 27; the first three changes alone took
  186 to 149.

**On a project** (`detangle_on_a_project`, ignored; release, `--ignored
--nocapture`, with `CCE_DETANGLE_PROJECT` naming a project directory or its
state.json, `CCE_DETANGLE_FRAMES` how far to play, 240, and
`CCE_DETANGLE_OUT` a directory for each way's last frame as an OBJ). The
file is read and never written: the detangle node inside its simnet is set
each way in turn in memory and the simnet played forward a frame at a
time on one cache, as playback does. It is how the node was first run on
something that was not a test's fixture — a 162-point icosphere with one
point pulled through its own far wall, relaxed behind it: as saved
(Points) the first crossing is at frame 20 and 35 edges are through a
triangle at frame 240, the pulled point a spike out of the far side;
Surface alone 71, later (frame 81); with Fold Contact 9; with everything
on none at any frame, the far wall carried out ahead of the point as a
tent, at 5.1 ms a frame against 0.5. Every row mattered there: the edges
alone and the folds alone each left crossings.

`detangle::self_intersections` is the MEASURE: every edge passing through a
triangle (`spatial::segment_crosses_triangle`, tolerance relative to the
lengths, so scale does not change the answer), no thickness and no rings.
`crossings_beyond(geom, rings)` counts only those the solve is meant to see
at a ring count, which is what separates a miss of the method from a fold
inside the excluded neighbourhood. The node's **Tangled Group** row, when
it names one, writes the points of what is STILL crossed after the solve
(empty when nothing is); it costs a second search of the mesh and is off
by default. `detangle_methods_compared` (ignored; release, `--ignored
--nocapture`) pushes an icosphere's cap down into its own bowl a fifth of
an edge a step. At 2562 points, Thickness 1, Rings 2: no detangle 1117
crossings, all beyond the rings; Points 1699 (892 beyond), 5.1 ms a step;
Surface 408, NONE beyond the rings, and none left at the end, 17 ms a step
(24 told where the step began: the wider search and the hold's look).
At four fifths of an edge a step and Thickness 0.5, where a step outruns
the thickness, Surface alone let 804 through beyond the rings and the side
brought that to 140 (642 points: 294 to 0).
At Thickness 0.5 Surface let 72 through beyond the rings at that size and
none at 162 and 642 points. What Surface leaves is the fold at the cap's
rim, inside the rings. (Those figures are with Edge Contact off, as the
test's `surface` and `sided` rows are.) Do not measure by pressing a sphere flat by the sign
of y: that carries the equator's points past their own neighbours, which no
setting is meant to see, and both methods look equally bad.

What still costs is the solver's, not the node's: an edit inside a simnet
re-solves from the seed, so a change at frame 120 is 120 steps. A
backward scrub no longer does — the next section.

### Where a simulation's time goes

`sim_profile_on_a_project` (ignored; release, `--ignored --nocapture`,
with `CCE_SIM_PROJECT` naming a project directory or its state.json and
`CCE_SIM_FRAMES` how far to play, 60) plays the project's simnet forward
as saved and again with each node of its chain bypassed in turn, so what
a node costs is what the solve saves without it. The file is read and
never written, and the disk cache is off for the run. On the project it
was written for (2026-09-29; a pull, a Surface detangle with every row on
and a remesh, 162 points growing to 525): 46 ms a frame as saved, 24
without the detangle, 6 without the remesh. The remesh was most of it
twice over, and three changes the same day brought the solve to 15 ms a
frame, 11 once the mesh is at rest, where what is left is the detangle's
own work at 524 points:

- **The flip pass keeps a valence table** (`remesh::flip_pass`), counted
  once and kept in step with the flips. It had asked `tris_of` for a
  point's valence eight times an edge, each a list gathered, sorted and
  counted: 9 ms of a remesh at 525 points.
- **The closest-point search begins at a quarter of a cell**
  (`TriGrid::closest`) and works a normal out for the winner alone. A
  query on the surface, which is what a remesh's projection asks, is
  answered from the cell it is in; it was 6 µs a query from the
  twenty-seven cells about it, 12 ms a remesh. Where a search begins does
  not change what it finds, since it ends only on a hit nearer than the
  box searched is wide.
- **A remesh settles.** The flip pass refuses a flip whose new edge the
  next split would cut, as the collapse pass always refused its own.
  Without the rule a long edge between two thin triangles was split and
  its midpoint collapsed into a corner — the edge turned to its short
  diagonal — and the flip pass, judging by valence alone, turned it back:
  93 edges split, collapsed and flipped at every step of a mesh that had
  stopped moving. And an iteration that finds nothing to split, collapse
  or flip, with no relaxation asked, ends the remesh; on the first the
  INPUT is handed back as it came, primitives and their order untouched,
  so the topology the detangle keeps its lists by is the same from one
  step to the next. The projection's grid is built when first wanted.

The first two are how a remesh is run and not what it does: the passes as
first written are kept under `cfg(test)` (`flip_pass_reference`,
`TriGrid::closest_reference`, `remesh_reference`) and
`the_remesh_matches_its_reference` holds the mesh to them bit for bit,
step after step. The third changes what a remesh makes, where a flip
would have made an overlong edge; `a_remesh_settles_and_then_leaves_the_mesh_alone`
is its test, and fails without the rule ("still changing 162 edges after
20 rounds"). `remesh::last_changes` is the count the test and the profile
read.

### Simulation checkpoints

A step is not invertible, so going back means going forward from
somewhere earlier, and until 2026-09-29 that somewhere was the seed: a
scrub from frame 120 to 119 was 119 steps, and dragging the playhead
backwards re-solved the whole history at every frame it passed. A solve
now keeps CHECKPOINTS in memory (`geometry::Checkpoint`, on the
`SimSolve` beside the latest state): a frame's state and what its last
substep consumed, which is everything a resume and the interior view
need.

- **One every `CHECKPOINT_EVERY` (10) frames**, kept as the solve passes
  it — and as it LEAVES it, which is the case that is easy to miss:
  played a frame at a time the solve is always asked for the very next
  frame, so it never passes a frame on the interval, it arrives on one
  and leaves from it.
- **The frame a backward scrub leaves is kept too**, so coming forward
  to it again is a resume.
- **A resume takes the nearest kept frame at or behind the one asked
  for**, the latest state included, so a scrub either way inside what
  has been solved steps fewer than an interval's frames.
- **They belong to one key.** An edit to the chain or the seed changes
  the key and they go with the solve they were frames of. An edit at
  frame 120 is still 120 steps: nothing earlier than an edit survives it.
- **Within a count and a budget** (`CHECKPOINTS_MAX` 48,
  `CHECKPOINT_BUDGET` 512 MB by an estimate of a state's size). With no
  room the SPACING doubles and stays doubled — what is off the wider
  interval goes, and what arrives after arrives that far apart. Not the
  oldest: a scrub is as likely to land near the start. And not every
  other one while new ones arrive at the old spacing, which is what the
  first cut did and which thinned the start of a long solve again and
  again until it had gaps of hundreds of frames.

What a resume arrives at is what a solve from the seed arrives at, state
and feedback both: `a_scrub_resumes_from_a_checkpoint_and_arrives_at_the_same_state`
compares them frame by frame and counts the steps each cost
(`SimCache::steps_run`). **Every evaluation goes through the one cache**
(`State::sim_cache`): the scene rebuild, the pull arrows, the params
pane's pickers, and since 2026-09-29 the spreadsheet and the
selected-group markers, which `sync_nodes` evaluates against a node
borrowed off `State` and which each used a throwaway cache for that
reason — a full solve from the seed at every refresh of either. The
cache is taken out before that borrow begins and put back after it, as
the scene rebuild takes it.
`the_spreadsheet_and_group_markers_share_the_sim_cache` counts the steps.

**The spreadsheet and the group markers follow the frame and upstream
edits** (`State::sync_selection_readouts`, since 2026-09-29). Their keys
are the selected node, its parameters, the geometry version and the
frame; they run from `sync_nodes`, from the end of every scene rebuild —
as the pull arrows do — and from the tick's frame change when the graph
holds no simnet and so nothing rebuilds. The spreadsheet's key had been
the node and its OWN parameters, and both ran from `sync_nodes` alone,
which a frame change does not call: during playback the spreadsheet
showed the frame it had been opened on, and an edit upstream of the
selection left it showing the values from before. A refresh keeps the
spreadsheet's scroll and sort. On the project this was measured on, a scrub
back over sixty frames from frame 120 went from a mean of 15 ms a frame
to 1.3, and from frame 240 from 67 to 4. The disk cache (`Cache` on the
simnet) is unchanged and still holds the one latest frame.

### Selected spreadsheet rows are marked in the scene

A row of the spreadsheet is a POINT of the node it shows, by index, and
rows can be selected (since 2026-09-29; the selection itself is cce-ui's,
see its CLAUDE.md, "A spreadsheet's rows can be selected"): a press
selects one, ctrl toggles one, shift extends a run. Each selected row's
point wears a marker in the viewport, in the highlight colour the row
wears and at Group Marker Size.

- **Nothing is evaluated by a selection.** `State::spreadsheet_points` is
  where the rows' points were when the table was last filled, kept from
  that evaluation; `rebuild_row_marker_verts` builds the markers from it,
  and runs from the press, from every refill of the table and from a
  change of Group Marker Size.
- **The selection stands across a frame and an edit**, since the rows are
  still the points they were, and the markers follow the points. It goes
  when the table becomes ANOTHER node's (`sync_selection_readouts`
  compares the node id), whose row 3 is some other point.
- **The markers draw only while the spreadsheet is shown**, with the
  group markers, before the geometry and at full opacity.
- A detached spreadsheet window has no viewport, and the selection does
  not ride the sync channel: rows selected there mark nothing in the main
  window.

`selected_spreadsheet_rows_are_marked_in_the_scene` drives it by pointer.

**The point groups are the spreadsheet's first columns** after the
point's number (`geometry_to_spreadsheet_data`, since 2026-09-29):
`group:<name>`, 1 for a member and 0 for the rest. They were `g:` columns
after every attribute, the thirteenth column of a sphere's table and off
the right of any pane, and blank for a point outside the group — so a
group of one point among five hundred was a column that looked empty.
Sorting the column descending brings the members to the top. Only POINT
groups: a row is a point, and the table has no primitive rows.

**What reads a selected node for display reads it as the scene shows it**
(`geometry::node_geometry_as_shown`, the same day): the spreadsheet's
rows, the markers on them and the selected group's. A node inside a
simnet is evaluated as the frame's last substep saw it, the feedback of
the nearest simnet above it pushed — the rule the dived-in scene walk and
the pull arrows already drew by. Until then these evaluated the node
bare, so inside a simnet `input` read the seed and the rows, and a
selected row's marker, stood at the first frame while the scene beside
them played. `rows_selected_inside_a_simnet_follow_the_simulation` is the
test.

### The volume representation

`src/volume.rs` is a dense signed distance field — `Volume { origin, voxel,
dims, data }` — with two nodes on it: `volume` (offset and shell) and
`boolean` (union, intersect, subtract). It exists because shelling, offsetting
and booleans are not mesh operations. Doing them on triangles means answering
"which side of this whole surface is that point on" per triangle pair; doing
them on a field means `min`, `max` and a sign flip, and the mesh comes back out
by extraction.

**Signing the field is the whole difficulty**, and it is done in two parts
because neither part is right everywhere:

- **Far from the surface**, a flood fill from the grid boundary — which is
  outside by construction — marks everything it can reach. Whatever it cannot
  reach without crossing the surface is enclosed, however convoluted the
  cavity. The flood may only step between samples that are *both* further than
  `voxel * 1.01` from any surface, because two samples one voxel apart cannot
  both be more than a voxel from a surface lying between them. A looser band
  (0.75 voxel was the first try) lets the flood walk straight through a thin
  wall and the solid comes back hollow.
- **Inside that band**, the flood has nothing to say, so the nearest face's
  normal decides. That test trusts the winding, so the winding is *measured*
  first — the signed volume by the divergence theorem, positive when faces look
  outward — and the test flips if the mesh is inside out. An imported mesh is
  not obliged to agree with this app's convention, and one that disagrees used
  to come back with its band signs alternating against the flood's.

Ray parity was the first approach and is wrong: a ray through a shared edge
crosses two triangles at one point and counts two, so the parity inverts for
every sample behind it. It failed on 79 of 15625 samples in contiguous runs,
which is what a parity bug looks like.

Extraction is naive **surface nets** (`to_mesh`): one vertex per cell that has
a sign change, placed at the average of its edge crossings, and one quad per
crossed grid edge joining the four cells around it. Chosen over marching cubes
because it produces quads on a quad grid and far fewer degenerate slivers.

One vertex per cell is also its limit. Where a feature is thinner than a voxel
— the knife-edge rim of a subtraction — two sheets of surface share one cell's
vertex and pinch, leaving edges with four faces. The result is still
watertight; it is not manifold. Hence two predicates on `Detail`, and the
difference matters: [`is_closed`](src/detail.rs) asks that every directed edge
have exactly one opposite (no boundary, consistently wound — what having an
inside requires, and what `Volume::build` guards its input with), while
`is_manifold` asks for exactly two faces per edge (what remeshing requires,
since an edge with four faces has no single pair to flip between).

`Volume::build` takes an explicit `reach`: distances are clamped there, so the
field is exact near the surface and flat far from it. A boolean builds both
operands on ONE grid so the two fields line up sample for sample.

### The 2D page context

`src/page.rs` is a second context, not a second kind of geometry node. Its
currency is a `Page` — an image: a physical size, a DPI, and straight-alpha
RGBA pixels — its origin is the top-left corner with y running DOWN, and
nothing in it has a point id, an attribute or a normal. Five nodes compose
one: `page` (the generator: preset or custom size, units, orientation,
resolution, colour, opacity, position), `page_grid`, `page_border`,
`page_text` and `page_shape`.

**The generator's size is in pixels or in real units** (since 2026-09-29).
The `page` node's `Units` row — Inches, Millimetres, Centimetres, Pixels —
is what its Width and Height are written in, and what EVERY node downstream
is written in: the page carries its `PageUnit`, and the resolver converts
each length through `Page::len` before it draws. A property of the page
and not of each node, because a chain whose text was placed in pixels and
whose border was inset in inches is a chain nobody can read. Inside, a page
is still inches (`Page::size`), and a pixel image's physical size is its
pixels over its Resolution. A node with no Units row is in inches, which is
every save from before it. The presets are the four paper sizes and three
raster ones (`HD`, `4K`, `Square`), which are their pixels whatever Units
says and are not turned by Orientation. The length rows are `float` — a
number with no range — where they were sliders over a range in inches: a
slider clamps, and no one range holds both 0.25 inches and 1920 pixels.

**`page_shape`** draws a rectangle (with a corner radius), an ellipse, a
line or a polygon of N sides, turned by Rotation, filled and stroked, each
with an opacity. Coverage comes from the signed DISTANCE to the outline in
pixels (`Page::shape`), so a turned edge and a circle's are clean lines; the
stroke is centred on the outline. A line is its stroke: as long as its
Width, as thick as its Stroke Width.

**The viewport shows the image, standing in the scene** (since
2026-09-29). The displayed page is uploaded as a texture and staged as a
`cce_ui::vk::SceneImage` — a textured quad in the 3D pass, unlit, depth
tested against the geometry, seen from both sides — in the XY plane about
the page node's `Position`, facing +Z, at its PHYSICAL size: the World Unit
says what one world unit is, and a sheet 215.9 mm wide is 215.9 of them
when that is a millimetre (`PageShown::world_size`, the one place a length
is converted INTO world units). It is staged after the furniture and the
markers and before the geometry, whose fill may be translucent over it, and
the shader discards a texel that shows nothing so a transparent page does
not hide what is behind it. The image is uploaded MIPMAPPED
(`cce_ui::vk::upload_rgba_mipmapped`, since 2026-09-29) and sampled with
anisotropy where the device has it: a Letter sheet at 300 DPI is drawn at
about a fifth of its size in an ordinary pane, and without a mip chain a
ruled page was moiré head-on and worse at a slant. `State::page_shown` is what the stage pass
places it by. Until then a pane of its own (`PAGE_IDX`, an `ImageView`)
took the viewport's rect whenever the level held a page, so a picture and
a model could not be seen together.

**The path tracer draws it too** (since 2026-09-29), in the viewport's
traced mode and in `--thumbnail`. The stage pass hands the tracer the same
upload at the same corners (`set_rt_scene_with_image`, a `cce_ui::vk::
RtImage`), and cce-ui adds the quad to the traced scene as two triangles
under a textured material — so both tiers meet it as any triangle, and a
scene that is an image ALONE is not an empty one, which the tracer used to
skip. Traced, the image is UNLIT, as the raster pass draws it: a ray that
lands on it takes the image's colour as it is and the path ends there, so
the two views show the same picture — and to the rest of the scene the
image is a light of its own colour, which is what a bounce off the
geometry finds there. (For its first hour it was a lit surface, albedo
under the sky, and a white page traced grey; the user's call.) Where it is
clear a ray goes through, by chance in proportion to the alpha. The traced scene's key is the geometry's version,
the image's (`State::page_version`, moved by every recomposition) and the
world unit, which sizes the image. The thumbnail has no 2D pass to share
an upload with and hands over the pixels (`RtImagePixels`); an image alone
is taken square on and fitted edge to edge (`thumbnail::view_of`), since
from the diagonal a picture is a slanted sliver of itself, and one with
geometry is inside the diagonal view's bounds. The denoiser leaves the
image alone: its pixels are marked as the sky's are, since a colour that
is the image's own has no noise to take out and smoothing took its fine
print first.

**The display flag is exclusive within its CONTEXT**
(`set_child_geometry_visible`): the page nodes and the geometry nodes each
have one, so a level shows one image and one geometry. One flag over both
is what made showing a picture hide the model.

**The image commands** (`src/image_tools.rs`, all registry rows):
`frame_image` (Ctrl+Shift+F, and a viewport-menu row while an image shows)
turns the active camera square to the image and fits it to the pane;
`view_image_pixels` does the same at the distance where one image pixel
covers one display pixel. A plane square to the view axis is scaled by a
perspective and not distorted, so head-on the image is exact. The Default
Camera is turned by setting its orbit to what cancels its base ray's own
yaw and pitch; a camera node has Position, Pivot and Rotation rewritten,
the Rotation taking up whatever orbit the viewport widget holds, which
`get_matrices` applies to every camera. Frame All holds the image's
corners beside the geometry. `new_image` adds a page node, shown and
selected; `add_image_rectangle` / `_ellipse` / `_line` / `_polygon` /
`_text` add a shape or text node wired after the selected image node (else
the shown one, else a new image), placed at the image's middle and sized
from it IN THE IMAGE'S UNIT, shown and selected. Added to the middle of a
chain the node is inserted: what read the target reads the new node.

**Shapes and text are placed by their handles** (`src/image_handles.rs`,
since 2026-09-29): the third and fourth `HandleSource`s, entered by Edit
Handles like the others and straight away by the `add_image_*` commands.
A shape has three — **move** (its middle), **size** (a corner of its box,
which grows about the middle) and **turn** (the middle of its right edge,
whose direction from the middle is the Rotation) — a line two, its middle
and an end that sets length and angle together; text has its anchor and a
handle one Size under it. They needed three things of the framework,
which the viewer-state section below describes: a `HandleCtx`, the
`drag` hook and the `plane`. The rows are written in the image's unit
through `PageFrame::row` — whole pixels, thousandths of anything longer.
`page::resolve_frame` is what makes the handles affordable: the page
WITHOUT its pixels, read off the `page` node up the chain, since the
overlay asks on every frame it is drawn and composing a sheet to learn
its size would be a sheet a frame. A drag still recomposes the image on
every motion, as dragging a slider does; `rebuild_page` keeps the GPU
image while the size holds (`update_pixels`), where it used to free and
upload one per rebuild and wait on the device each time.

The two contexts do not mix, and `is_page_node` is the one place that says so.
A page node contributes nothing to the viewport's geometry and a geometry node
cannot feed a page: page chains resolve through `resolve_page`, never through
`generate_single_node_geometry_with_errors`. `export` is the only node in
both — it passes either through, and what reaches it decides the format, so a
page writes a PNG and geometry writes the mesh format its Format parameter
names. There is no PNG option on that parameter, because offering one for a
mesh would be a lie.

**Resolution is a property of the page, not of the export.** The raster is
size × DPI, and `write_png` puts that in the pHYs chunk, so a printer lays the
file out at the size it was composed at instead of guessing 96. pHYs is pixels
per metre — the only unit PNG offers — so the DPI round-trips through a
conversion and comes back a hair off (300 stores as 11811 px/m, reads as
299.9994).

Rect coverage is exact area, not a test of the pixel centre. A printed grid is
mostly hairlines, and a binary fill snaps every rule to whole pixels, so a
ruled sheet comes out with lines alternating between one and two pixels wide
down its length — which reads as a wobble in the paper rather than as
aliasing. Grid rules are centred ON their coordinate so a second grid at twice
the cell size lands exactly on the first's, which is the only reason to draw
two. Text shapes and rasterizes through cosmic-text, the toolkit's own font
stack, with system fonts loaded because a page names its font by family.

The GPU image is owned by `State::page_image` and freed when replaced.
**A replacement renderer invalidates that id.** There is no reconnect
callback: the runner calls `renderer_init` once per renderer, so the first call
is this process's own and every later one is a replacement — remembering is the
only way to tell them apart (`State::seen_renderer`, via
`renderer_handed_over`, which is split out of the callback so it can be tested
without a live `VkRenderer`). Images uploaded outside that callback are not
replayed, so a cached id names nothing and its draws are skipped in SILENCE:
the image just goes from the scene. The id is dropped and `page_dirty` asks the next
tick to recompose and re-upload — the raster is cheap to rebuild from the node
graph, and no id can be carried across renderers. Found by cce-1f's audit of
clients caching vk image ids.

**To test it**, put `CCE_UI_FAULT_RECONNECT=<seconds>` on the binary's
environment: the runner drops the session that many seconds in, exactly as a
transport error would, and the app reconnects with a new renderer. Run the OLD
binary through the same fault first — a fix that passes a test which never
reproduced the bug is worth nothing. Judge by the picture: the designer inits no
logger, so the runner's WARN never appears even when it fired. Verified this way
on 2026-09-19 — with the fix disabled the sheet vanishes at the fault, with it
the sheet survives.

`gem_graph`, the source family's
everything-at-once node, is deliberately not ported: it is these nodes chained,
and that collapse is the whole premise of "fifty operators, ten nodes".

### The pane plates: one material, one relief block

Every plate the designer draws — the network panel, params, spreadsheet,
playbar, and every collapsed stub — goes through
`append_widget_plate_radii` in `render.rs`, and every one of those widgets
answers `color()` with `cce_ui::colors::param_plate_fill`, which is
`Material::pane()`: the toolkit's PANE rung. So there is ONE material for
the designer's plates, configured in the `style.surface` block of
`~/.config/cce/config.kdl` (a `~/.config/cce/cce-designer/config.kdl`
merges over it key by key, when one exists), and the network plate and the
params plate cannot be styled apart short of binding a named material
(`plate material="…"`). The viewport has no plate of its own: its lip is the
window's root-plate edge. What the block looks like after the 2026-09-28
consolidation, and what each key is:

```kdl
style {
    surface {
        plate {
            pane color=(rgba)"#6c6c7bf2"         // the tint, alpha = strength (legacy: param.color × plate_opacity)
            frost radius=(f64)5.5 compression=(f64)0.0 refraction=(f64)0.0   // the one frost spelling
            border_color (rgba)"#9595a9ff"        // the flat border, relief OFF only
            border_thickness (f64)1.0
            root { corner_radius (i64)24 }        // the pane corner radius falls back to this
        }
        relief light=(f64)0.15 width=(f64)9.3 shader=(bool)true {  // light: strength, NOT a length; width: the one roll/wall run;
            // shader: false = the legacy banded edge lighting, an A/B switch (was window_manager.bevel_shader)
            wall height=(mm)0.3 profile="smooth;…"   // a carve's wall: buttons, wells, param rows
            edge height=4.0    profile="smooth;…"   // a plate's perimeter roll: these panes
        }
        menu color=(rgba)"#101018ff"             // the dialog and every context menu (below)
    }
}
```

The rules that took a day to settle, each with the wrong version it replaced:

- **One roll width.** `relief.width` is the run of every roll and wall: the
  root plate, a `PlateSpec` pane plate, these bordered widget plates (through
  `colors::plate_bevel_width`), every control wall, and the length
  `edge.height` is a rise against. Until 2026-09-28 the widget-plate path
  had `style.surface.plate.bevel_width` of its own (default 6 against the
  relief's 9.3), so the panes here and the window lip rolled over different
  widths and no single key made them match. The old key was an override
  for the rest of that day and is retired: reported by path at load with
  the other retired surface keys, not read, removed by cce-relief's Save.
- **Wall and edge are two shapes, and the config names them.** A wall is
  shaded as a translucent overlay on what is under it; an edge multiplies
  the plate's own fill and adds a specular crest. Each node carries
  `height` (a length — the wall's drop, the roll's rise; unset = follow the
  width) and `profile` (a ramp spec; absent = the analytic curve). The flat
  spellings (`height` / `profile` for the wall, `edge_height` /
  `edge_profile`) and `depth`, the strength's former name (`light` now),
  are retired: reported at load, not read, rewritten by cce-relief's Save
  from a one-time seed. So is the whole `window_manager.bevel_*` block the
  relief was born in — `bevel_depth`, `bevel_width`, and `bevel_shader`,
  which is `relief.shader` now and takes a `(bool)`.
- **cce-relief's knobs are not a style key.** The Shoulder / Base / Bias
  triples behind each profile (`wall.knobs` / `edge.knobs`, before that
  `profile_knobs` / `edge_knobs`) were editor state beside the values that
  draw; they live in `~/.config/cce/cce-relief/state.kdl`, one node per
  Save target. A config still carrying one seeds the editor once, and its
  next Save takes the key off.
- **Frost is one block, and the flat keys are retired.** `plate.blur`,
  `.radius`, `.backdrop_compression` and `.refraction` are reported by path
  at load and not read — `blur=true` alone is a SHARP plate, and the warning
  is what says why. A named material's `frost` child spells its knob
  `compression` too.
- **Focus is the bevel's tint.** With relief on (`window_manager.control_relief`,
  default on), a plated pane marks focus by tinting its roll with
  `highlight_primary_color` (`plate_focus_tint`); the flat `border_color`
  draws only with relief off.

Two keys look like they apply and do not: `style.surface.plate.color`
feeds `plate_color`, whose one consumer is the info box, and the finish's
spec / shininess / curvature live as `relief.spec` / `.shininess` /
`.curvature`, not under `plate`. The network pane is two layers: the
`PassivePlate` above and the Graph on top, which has NO fill of its own
(since 2026-09-29): its cells are whatever it is painted on — the pane
plate, or the scene with the plate off — and `style.surface.graph` sets
only the lines (`grid_color`, `line_width`), their `opacity`, and `blur`.
Until then the graph filled itself with `cell_color` (chosen by a
`uniform_background` flag this app hard-coded true) and drew its lines in
`gap_color`; all three keys are retired, reported by path at load and not
read.
The next section is about dropping the pane plate entirely.
The params widget alone also reads `style.surface.param.backdrop_compression`.
The rules live in cce-ui's CLAUDE.md ("There is one roll width", "The
relief is two shapes", "Frost is one block"); this is the designer's view of
them, written because the question "what are the style parameters of the
plates" took a session to answer from the code.

### The network plate is optional

The network pane can drop its PLATE — the filled, frosted surface its graph
sits on — so the nodes and wires overlay the 3D scene directly. The viewport is
full-bleed (`CANVAS_IDX` covers the window and the other panes float over it),
so removing the plate is all it takes: what is behind the pane is the scene.

The pane itself is untouched. It keeps its rect, its focus domain, its corner
menus, its clip and its keyboard navigation; only two `append_widget_plate_radii`
calls are skipped — `CONTENT_IDX`'s (the graph's own plate) and
`NETWORK_PANEL_IDX`'s (the panel behind it). Skipping one and not the other
leaves a surface, so both are gated on the same flag. Node bodies keep their
blur-behind fill, which is what makes the result legible: they frost the scene
behind each node while the gaps stay clear.

**With the plate off the pane spans the whole window.** The dock rect is
overridden at its source in `rebuild_positions` — one `let (px, py, pw, ph)`,
so content, panel and breadcrumb all follow — because there is no surface left
to bound the graph, and one confined to a rectangle you cannot see is worse
than one that spans what it is drawn over.

That makes the pane's RECT useless as a hit test, and three things route off it:

- **Clicks** ask `in_network_pane`, which in overlay mode narrows to "a node is
  under the cursor, and no floating pane covers it" (`overlay_claims`). The
  same refinement goes into the press cascade's `hits_widget` closure, where
  the circular pane already refines its own hit test. Without it the graph
  claims every press in the window, including ones landing on a node drawn
  UNDER the params pane.
- **Pan gestures** ask `in_network_area` instead — the plain rect. Middle-drag
  and space+left mean nothing to the scene, so the network keeps them across
  its whole span; a graph you could not pan by dragging because its own surface
  stopped being drawn would be a strange thing to ship.
- **`cursor_in_viewport`** becomes the complement: the body, minus what the
  network holds, minus the floating panes.

What changes for the user: a plain click on empty space is no longer the
network's — it ORBITS THE CAMERA instead (see below), which is what makes the
overlay feel like a scene with a graph on it rather than a graph with a
picture behind it. Deselecting is on Escape.

### Deselecting has to stick

The selection IS whatever sits in the grid cursor's cell — that is what
`sync_cursor_and_selection` means — and that sync runs on nearly every frame
where anything changed. So `set_selected_node(None)` alone does not deselect:
it is put straight back on the next frame, and the pane never clears.

`State::deselect_node` therefore remembers the CELL it happened in
(`deselected_cell`), and the sync leaves that one cell alone. A cell rather
than a flag, so the suppression is exactly as narrow as it should be: step the
cursor anywhere else and selection resumes by itself, and stepping back onto
the node selects it again. A selection arriving from anywhere else — a click, a
load, the params pane — spends the memory at the top of the same sync, or
clicking the very node you just deselected would clear itself again.

Escape runs it LAST, after the context menus, the viewer state and
connection-cancel: Escape is this app's one "get me out" key, and all of those
are more immediate than a selection. There is also a `deselect` command, shipped
UNBOUND so it is findable in the palette — deliberately not Ctrl+D, which the
plugin uses for deselect-all but which this app already gives to Circular Pane.

`ViewportSettings::network_plate` persists it, beside the viewport toggles
rather than in the project's pane-state list: a pane's VISIBILITY belongs to
the project, but whether its surface is drawn is how you like to work, and it
should outlive any one file. It is reachable three ways that cannot disagree,
because all three run one `Action::ToggleNetworkPlate` — the View settings
node's Network > Plate row, the network pane's View menu ("Network Plate"), and
the `toggle_network_plate` command. The action marks `settings_changed` and
lets `execute_action` save once at its end, like every other viewport toggle,
rather than writing the file itself.

### The network editor's right-click menu

A right press on EMPTY graph space opens the network's own context menu; a press
ON a node still opens that node's menu, which is the more specific thing under
the pointer. Until 2026-09-22 the empty-space press opened the **add-node
palette** outright, which left the network the one pane whose right-click was
not a context menu, and left every other graph-wide command reachable only by
chord or through the palette. **Add Node is the first row** instead, and picking
it opens the same palette.

Rows are `NETWORK_MENU_COMMANDS` — a list of COMMAND IDS, `None` for a
separator — resolved through `command::by_id`, so a label is the registry's
label and `NetworkMenuAction::Command(id)` dispatches through `run_command`.
The menu therefore cannot name work the palette spells differently, and a row is
exactly as scriptable as the command behind it.
`network_menu_rows_name_commands_that_exist` is the backstop, since a row whose
id no longer resolves is simply skipped. A toggle command carries the viewport
menu's `●`/`○` mark, read through `command_toggle_state` — the one table the
dialog's switches read too.

`add_node` is a registry row of its own now (`Run::Menu("Add Node")`), where the
palette used to be reachable only from Tab's inline handler. It ships UNBOUND,
like `deselect`: Tab already opens it from the event loop, and a default chord
here would duplicate a key the loop claims.

With the plate OFF the press never gets here — `in_network_pane` narrows to the
nodes in overlay mode, so empty space is the scene's and opens the VIEWPORT
menu. That is the overlay's whole rule, and it predates this menu.

The press moves the grid cursor to the clicked cell BEFORE the menu goes up,
because that cell is where Add Node will place what it adds — the cursor is the
only thing carrying the pointed-at cell across to the palette.

### Keyboard graph navigation

The network pane's keyboard scheme is the plugin's, ported: **hjkl rather than
arrows** — the arrows are the playbar transport in every pane and context — bare
to move the grid cursor, `shift` to extend it into a region, `alt` to move the
selected nodes, `ctrl` to pan the view, plus `f` to frame the cursor and
`shift+f` to frame everything. All eighteen are registry commands in
`Context::Network`, so they are rebindable through `input.kdl` and listed in
the palette.

**The grid cursor IS the selection.** `sync_cursor_and_selection` selects
whatever node sits in the cursor's cell, so navigating selects, and stepping off
a node deselects. Every family is gated on the network pane having focus — one
gate, in the four `network_*` methods. The bare family used to be the one that
was NOT gated: plain h/j/k/l moved the cursor from any pane, so it drifted
invisibly while you were looking at the viewport (the selection did not follow,
because `sync_cursor_and_selection` has its own pane check) and was somewhere
unexpected when you came back.

`alt` moves the SELECTION and the cursor, so a run of `alt+h` drags what is
selected across the sheet rather than leaving it behind on the first press. `ctrl` pans by one CELL
rather than a fixed pixel count, so a pan step means the same thing at every
zoom. Frame Cursor CENTRES the cursor cell; its first version called
`keep_cursor_in_view`, which pans only when the cursor has gone off an edge, so
the command did nothing at all in the common case of a cursor that is visible
but off in a corner — which is exactly when it gets pressed.

`shift+hjkl` — the plugin's extend-the-selection family — grows the cursor's
region (below) by one cell. It was absent while the graph's single
`selected_node` was the whole selection, when four rows would have done what
bare hjkl already does; there is a real multi-selection to extend now.

**The anchor never moves.** `network_extend` walks the region's FAR corner and
leaves the anchor where it is, exactly as a drag does, so `shift+l` then
`shift+h` returns to where it started rather than walking the whole region
right and back. A far corner that meets the anchor again drops the expanse
outright, so a region shrunk to nothing is the plain one-cell cursor and not a
1×1 region that merely behaves like one — and carrying on past the anchor grows
it the other way. Extending from a cursor that sits ON a node keeps that node
selected, the anchor's cell being part of its own region, which is what makes
the family an extend rather than a second way to start a selection.

It scrolls the FAR cell into view (`keep_cell_in_view`, which
`keep_cursor_in_view` is now a one-line wrapper of): the anchor is the end that
is not moving, and following it would scroll the wrong end of the selection
into view.

**Frame All fits the name labels, not just the bodies.** A label hangs off
its node's right edge (`Graph::node_labels`: an 8 px gap and a 14 px font,
both scaled with the body against its 80 px baseline, the font clamped to
6..48), so framing the bodies alone cut the right-hand column's names off.
`State::node_extent` repeats that rule — the widget offers no query for it —
using the widget's own `TextLabel::estimate_width`, the number it culls the
label against, so the two cannot disagree. The fit is iterated rather than
solved once, because the extent is not linear in the zoom: the font floor and
the width's rounding mean a fit computed at 100% overstates what a small zoom
saves. `frame_all_keeps_the_node_labels_inside_the_pane` is the check, and it
fails on the body-only fit.

Two chords moved to make room, both caught by `command::conflicts` rather than
by hand: `edit_handles` from `Ctrl+H` to `Ctrl+Shift+H` (the ctrl+hjkl family
owns those now), and `f` now frames the CURSOR where it used to frame
everything, with framing everything on `shift+f` — the plugin's split.

### The network grid is a lattice, and a node sits on a crossing

The network grid has ONE size per axis: `style.surface.graph.spacing_x` /
`spacing_y` in config.kdl, the pitch — the distance from the centre of one
grid line to the centre of the next. A node's `position` (col, row) names a
lattice intersection, and the node body is CENTRED on it. The body has a
size of its own, `style.surface.graph.node.width` / `height`, which the
pitch does not touch: a denser grid moves nodes closer, it does not shrink
them (a first cut derived the body from the pitch; it was disconnected the
same day). Until 2026-09-22 the grid was rounded CELLS with grout between
them, configured as a cell size (also the node size) plus a gap, and a node
filled its cell.

**Which file sets the pitch is easy to get wrong.** cce-ui merges the
per-app override `~/.config/cce/cce-designer/config.kdl` OVER the main
`~/.config/cce/config.kdl`, key by key, so a `spacing_x` in the per-app
file wins over any edit to the main one — a whole afternoon of "the grid
size is not changing" (2026-09-22) was a stale `spacing_x=71` in the
override, left from the cell model. `get_state` over MCP reports `grid`
(the live pitch and node size, the zoom percent, and the CONFIGURED pitch
and node size), which is the one way to check from outside that a config
edit reached the lattice.

`State::grid_pitch_x` / `grid_pitch_y` and `node_w` / `node_h` are the
zoomed geometry — `configured_grid_geometry` at 100%, scaled TOGETHER by
`scale_grid_geometry`, which is the only relation between them; there is no
other grid geometry on `State`. `cell_center`, `cell_rect` and `cell_at` are
the three derivations every consumer goes through — the cursor outline, the
click-to-cell of an empty-space press (`round`, not `floor`, because a cell
is centred on its crossing and a click between two nodes belongs to the
nearer), Frame Cursor, the zoom anchor. The configured geometry is the 100%
baseline Reset Zoom returns to and Frame All scales down from (never past
100%); `MIN_PITCH_*` / `MAX_PITCH_*` are the old node-body zoom limits
expressed on the pitch.

The widget paints the lattice as lines (`paint_grid`: `grid_color`, network
opacity, `style.surface.graph.line_width` px, each line centred on its
coordinate so the width changes nothing about where anything sits) with the
two lines through the (0, 0) crossing heavier as the origin axes. Its
cell-and-gap setters (`set_grid_sizes` / `set_skipped_sizes`) survive as a
description of the same lattice for cce-files and cce-graph, which still
speak it; this app sets the pitch.

### The cursor is a region, and dragging the grid grows it

A left press on EMPTY grid puts the cursor on the pressed cell — on the press,
not the release — and arms an expansion drag from it. Dragging grows the cursor
from that anchor to the cell under the pointer. `State::grid_cursor_region` is the one derivation,
`(col, row, cols, rows)`, never smaller than one cell; `grid_cursor_rect` is the
window-space union the outline is painted on, which for the usual one-cell
cursor is exactly `cell_rect` of it.

**The release SETTLES the region** onto what it caught
(`settle_cursor_expansion`): the bounding box of the selected nodes, or — with
nothing caught — one cell at the MIDDLE of where the region stood, even spans
rounding down toward its first cell. A region is a way of pointing at nodes,
and once the pointing is done the empty margin the pointer swept through is
noise: it hides nothing, it selects nothing, and it leaves the next alt+hjkl or
Add Node reading off an anchor out in open grid. Settling also makes the region
say what was selected — a box drawn loosely around two nodes comes back fitted
to them. Nothing caught settles to the middle rather than back to the anchor,
because the anchor is merely where the gesture began and a drag that selected
nothing is aimed at the space it ended up circling.

The SELECTION never changes in a settle — the bounding box of the selected
nodes contains no cell the region did not — which is what lets it run at the
end of every drag without a thought for what it might drop.

**The region collapses by itself.** `grid_cursor_expanse` stores the anchor
alongside the far cell, and `grid_cursor_region` hands it back only while that
anchor is still `(grid_cursor_col, grid_cursor_row)`. So every OTHER way the
cursor moves — a nav key, a click, a load, the selection following a node —
leaves the anchor behind and drops the region with it, without a line in any of
those places. Fifteen call sites write the cursor; a flag reset by hand at all
of them is a flag that gets missed at one, and a cursor left stretched across
the sheet is not a subtle wrong.

**An expanded cursor selects every node standing inside it.**
`State::selected_slots` is the selection, and it has two arms for a reason:
one cell — the ordinary cursor — DEFERS to the graph's own `selected_node`,
so nothing about a single selection changes (that one answer already carries
the deselect memory, a click that arrived from another pane, and a selection
made while the network was not focused); an expanded cursor names every node
on a cell it covers instead. Its anchor is empty grid by construction — a
press on a node drags the node — so there is no single selection to defer to.

The network's operations act on that selection: **Delete**, the **`e`**
geometry toggle, **Ctrl+C/X**, **alt+hjkl**, and the **mouse**. Two rules worth keeping:
deletions run HIGHEST SLOT FIRST, or removing one shifts the slots above it
and the second removal takes the wrong node; and the `e` toggle sets the whole
selection to the opposite of the FIRST node's flag rather than flipping each,
because a toggle over a mixed selection should settle it, not shuffle it.
`network_move_node` moves the region along with the nodes — stepping the
anchor alone is precisely what collapses a region, so the first alt+h would
otherwise drop the selection it had just moved. The clipboard is a `Vec`, and
a paste keeps the SHAPE it was copied in: the set's top-left lands on the
cursor and each node keeps its offset, with a node whose cell is taken
stepping aside to the nearest free one.

**Dragging a selected node carries the whole selection** (`NodeDragGroup`,
`drag_group_to`). The widget drags ONE node — it has one `dragging_idx` — so
the companions are moved here, rigidly, by the offset the dragged node has
travelled, measured from the cells they started on rather than stepped each
frame (a drag is continuous but resolves to whole cells, so accumulating the
steps would drift the group apart the first time two motions named one cell).
They are NOT walked off occupied cells the way the widget walks the node it
drags: a selection that rearranged itself around whatever it passed over would
not be the selection you picked up — the same bargain alt+hjkl has always made.
The preview follows `drop_target_cell_rect`, which runs `commit_drag`'s own
resolution, and the release re-lays them from the cell that actually committed,
since the widget can walk the dragged node a cell aside from the preview.

Two things make that gesture work at all. **A press on a node inside the
selection leaves the cursor alone**: the press path otherwise moves the anchor
onto the pressed node, which is exactly what collapses a region, so the
selection would be gone before the drag began. A press on a node OUTSIDE the
selection does move it, and that collapse is the right one — clicking an
unselected node selects that node. And `read_panel_offsets` returns early while
a group drag is live, for the same reason: it yanks the cursor onto the
selected node's cell, and the anchor is deliberately standing still. On release
the region is shifted by the committed offset, as alt+hjkl shifts it.

Escape collapses the region (`deselect_node`), because of the two selections
this is the one that needs clearing: a single selection under a plain cursor
comes back on the next sync anyway, while a region stands until the cursor is
moved off its anchor.

Everything that reads the cursor as ONE CELL still reads the anchor: Add Node
places there, Frame Cursor centres it, `sync_cursor_and_selection` sets the
graph's own selection from it. That single selection is deliberately NOT set
from the region: `read_panel_offsets` yanks the cursor onto the selected
node's cell, which would move the anchor off its own region and collapse it
on the next layout sync. So with a region up the params pane shows nothing —
it shows one node's parameters, and the selection is many.

The paint reads the same `grid_cursor_covers`: a node body is recognised by
the cell it is centred on and drawn with the highlight tint the widget gives
its own single selection, rather than by a second rect test that could
disagree with the selection itself.

Arming is gated on the graph NOT having taken the press (`widget_took`). The
case that bites is a press on a PORT: it starts a connection and consumes the
press without selecting anything, so the empty-grid arm would read it as bare
lattice and then swallow every motion event — leaving the rubber-band line
frozen at the port it started from. The gesture is otherwise uncontested,
because `Graph::draggable` is true only while it is moving a node.

### Auto-layout

`src/layout.rs` arranges a level's nodes from their wiring. The network is
already a GRID — positions are integer cells and the keyboard cursor steps cell
by cell — so this is a layered assignment on cells, not a force-directed
sprawl: a node's ROW is how far downstream it is, its COLUMN is chosen to sit
under what it reads from.

**Edges come from the same rule the wires do** — a node's `Input` parameter
naming another node, which is the widget's `wire_pairs` derivation. Matching it
is the point: a layout computed from relationships you cannot see would move
nodes for reasons that are not on screen. It also means a second operand (a
Boolean's `With`, a Copy's target) does not pull on the layout, because it does
not draw a wire either. When those become wires they should become edges here
in the same change.

Flow is downward, matching every project in the repo (a Sphere at (4, 2)
feeding an output at (4, 3)). Row is the LONGEST path from a root, not the
shortest, so a node always sits below every one of its inputs rather than
beside one of them. Depth is computed by iterating to a fixed point rather than
by recursion, because a name-wired graph can be cyclic — A reads B reads A is
something a user can type — and the loop stops improving instead of
overflowing the stack.

Utility trees are pinned: the settings node lives where the user put it, and an
"arrange everything" that relocated it would be a surprise every time. Their
cells count as occupied so nothing lands on top of them. Within a row, a node
wants its parent's column (a root wants the column it already has, which
preserves the left-to-right order among independent chains) and takes the
nearest free column to that, searching outward — so a chain stays perfectly
vertical and a collision nudges one node aside instead of shifting the whole
row.

`arrange` returns only the nodes that MOVED, so `layout_current_level` can say
"moved 3 nodes" or "every node was already in place" — an arrange that did
nothing because the layout was already right looks identical to a broken one,
and the status line is the only thing that separates them.

The command is `layout_nodes` on `Ctrl+Shift+L` rather than the bare `L`
Houdini uses: bare hjkl is the cursor, and shift+hjkl is reserved for the
select family this app cannot implement until the Graph widget has
multi-selection, so taking `Shift+L` now would have to be given back later.

### Display mode: the viewport menu, and smooth shading

The viewport's right-click menu has two SUBMENUS (since 2026-09-29;
until then it was one list of some twenty rows). The menu itself holds
what is done — Frame All, View 1:1 — the guides (Show Grid, Show Origin,
and since 2026-09-29 Show Camera Pivot with a Camera Pivot Size slider
under it, 0–1 by a twentieth (the palette row is the coarse one, whole
tenths up to 5; the size is the LENGTH of the marker's beams, whose
thickness is fixed), which until then were in the Guides
menubar and the palette only;
the reference CUBE guide was removed on 2026-09-25 — its command, mesh,
RT-scene copy, settings field and menubar item, with the Guides menubar
addressed through `GUIDES_MENU` / `GUIDE_*` so no item slid onto another's
action, while old files carrying `show_cube_enabled` still load), and a
row for each submenu: **Style** (how the geometry is drawn: the
wireframe's switch, thickness and opacity, then the surface's shading,
opacity and Show Occluded) and **Markers** (what is drawn on it: Group
Marker Size and Pull Arrow Scale; then the
overlays a class at a time — Show Point Markers and its size, Show Point
Numbers, Show Point Normals; Show Primitive Numbers, Show Primitive
Normals; Show Vertex Markers, Show Vertex Numbers, Show Vertex Normals).

**The submenus are cce-ui's** (`context_menu::set_row_submenu`, see its
CLAUDE.md, "A row can open a submenu"): a second menu in its own popup
that flies out beside the row under the pointer, which the toolkit opens,
closes and places. What this app does is fill and dispatch.
`viewport_menu_rows_of(page)` is the rows of the menu (`None`) or of a
submenu; `fill_viewport_submenus` hands each `ViewportMenuAction::Submenu`
row its rows and sliders, read from the live state, at the open and again
after anything a submenu row did — the toolkit changes an open submenu
where it stands, which is how a mark follows its switch — and
`open_viewport_submenu_actions` is the open submenu's actions, by
`submenu::parent_row`. **A row of a submenu keeps both menus up**: a
submenu is a panel of settings, opened to set several, so they close on a
press outside or Escape; a row of the menu itself closes it as before.
The slider hooks are unchanged, the toolkit's pointer calls answering for
both menus; `drain_viewport_menu_slider` drains the menu's sliders and
then the submenu's. For one day the two were PAGES of the one popup,
entered by a row and left by a Back row, because the toolkit had no
submenu. `the_viewport_menu_flies_its_settings_out_beside_it` drives it
by pointer.

**The primitive and vertex overlays** (`toggle_prim_numbers`,
`toggle_prim_normals`, `toggle_vertex_numbers`, the same day) are
collected by `render::scene_element_overlays` off the scene `Detail` the
point overlays read: a primitive's number at its centroid, its normal
(Newell's, so a quad not quite flat has one) a whisker from there, and a
vertex's number — its index in the detail — inset `VERTEX_LABEL_INSET` of
the way from its point toward its primitive's centroid, so the vertices
sharing a point stand apart, each inside its own primitive. Warm for
primitives, green for vertices, the points' pale blue unchanged. They are
persisted beside the point overlays in `ViewportSettings`, capped at 2000
labels a class, and dimmed by the fill in front of them as the point
numbers are (one `point_transmittance` call over all three lists).
`primitives_and_vertices_are_numbered_where_they_are` is the test.

**Vertex markers and vertex normals** (`toggle_vertex_markers`,
`toggle_vertex_normals`, later the same day) stand where the vertex's
number does, so all three of a vertex's overlays name one place. The
markers are the point markers' spheres at `VERTEX_MARKER_SCALE` (0.6) of
Point Marker Size, in the vertex green, smaller so a point's marker is not
lost among the markers of the vertices around it; they share the point
markers' mesh, and `rebuild_overlay_marker_verts` builds both lists, so
the size slider re-sizes both without an evaluation. **A vertex's normal
is its `N` attribute where the detail carries a Float3 one on its
vertices, and its primitive's normal where it does not** — the normal of
this corner of this face, where a point's is the average over the faces
around it. Without the attribute the whiskers show the faceting: the
corners of one face agree, and a shared point wears a fan of them. `scene_element_overlays` takes an `ElementOverlays`
of what is wanted and returns an `ElementOverlayGeometry`.

`the_viewport_menu_groups_its_display_rows` holds the
order of all three. The rows: the Show Wireframe switch (its registry command), **Flat
Shading / Smooth Shading** as a radio pair over `toggle_smooth_shading`,
a **Wire Thickness** slider under the wireframe switch (1–8 px by
half a pixel, the palette row's range), a **Wire Opacity** slider under
that (percent by 5, `State::wire_opacity` — the wires' own, apart from the
polygons' Opacity; until 2026-09-25 it was the Wire Color's ALPHA, and
`StoredRenderSettings` moves an old alpha, from state.kdl's `#rrggbbaa` or
a project's four-component array, into it on load), a
**Point Marker Size** slider (0.005–0.1 world units, the palette row's
since 2026-09-29 — until then that row was a spin in THOUSANDTHS, so the
two marker sizes read as different numbers for one radius, and 0.025
typed into it landed on the spin's floor; no suffix since
the World Unit names the units), a **Group
Marker Size** slider (0–0.2 world units by 0.005), and the
polygon **Opacity** as a
SLIDER row — cce-ui's `context_menu::MenuSlider` (2026-09-25), set on the
shown menu by `open_viewport_context_menu`. `viewport_menu_slider` is the
one table of the menu's sliders (read from the live value) and
`land_viewport_menu_slider` writes each back, so another slider row is a
row in `viewport_menu_rows` plus an arm in each. The wheel over Opacity steps 5% and saves; a press
on its band jumps and drags (a slider row never closes the menu), landing
the value live and committing on the release. The designer dispatches the
menu itself, so four hooks carry it: `slider_press` ahead of the row action
in `handle_viewport_menu_click`, `slider_dragging` in CursorMoved (after
`cursor_moved`, which moves the held value), `slider_release` at the top of
MouseInput, and `mouse_wheel` at the top of MouseWheel — where a wheel
anywhere over the open menu is swallowed rather than orbiting the scene.
`drain_viewport_menu_slider` lands a change through
`land_viewport_menu_slider`, which sets the live field and redoes only what
it feeds: opacity and wire thickness are draw-time, and group marker size
re-bakes the Selected-Group
markers — re-sized from `State::group_members`, the positions `sync_nodes`
keeps from its evaluation, by `rebuild_group_marker_verts`; point marker
size re-sizes the Show Point Markers overlay from the scene positions
`rebuild_scene_geometry` keeps while it is on (`overlay_marker_points`,
`rebuild_overlay_marker_verts`). None of it
re-evaluates the graph, which `apply_setting`'s regenerate pass would do per
pixel of drag. `sync_nodes` also re-sizes the markers when only the size
moved (`last_group_marker_size`), so the palette's Group
Marker Size row reaches them the same way. `viewport_menu_rows`
and `run_viewport_menu_action` are split from the open and the click so a
test reads and runs the rows.

**There is one display of a marker on every point, Show Point Markers**
(since 2026-09-29). Until then there was a second, **Show Points**
(`toggle_render_points`, with Point Size and Point Color), which the
retired Render node had brought: the same small sphere on the same points
of the same scene, with a size and a colour of its own, differing only in
following the fill's Opacity. It is gone — the command, the mesh, the three
settings. The group markers were sized off it, Point Size times Group
Marker Scale, and have a size of their own now
(`State::group_marker_size`, world units); `StoredRenderSettings` reads a
file from before by multiplying the old pair out, and does not read
`render_points` or `point_color`.
`an_older_render_block_gives_the_group_markers_their_size` is the test.

**Smooth shading is baked, not shaded.** The raster pass flat-shades every
fill in `scene3d.wgsl` from screen-space derivative normals, and cce-ui's
`Vertex3D` carries no normal. The light is fixed in WORLD space, though, so
lighting each vertex from its smooth point normal and interpolating is
exact: `geometry::smooth_lit_vertices` multiplies each corner's colour by
`shade_factor(point_normals[p])`, and the fill draws with
`SceneDraw::prelit` (cce-ui, 2026-09-24) so the shader does not shade it
twice. `shade_factor` has to agree with the shader about which side is
lit: the shader's normal is screen-right × framebuffer-DOWN, which for any
visible surface points AWAY from the viewer — into the surface — so the
bake uses `dot(-n_outward, l)`; on a plane the two modes give identical
brightness (`smooth_shading_bakes_the_flat_shaders_light_per_vertex`). The
lit copy is `State::scene_smooth_verts`, kept only while smooth is on; the
path tracer keeps reading the unlit `rt_sphere_verts`, whose colours are
its materials. Smoothing follows topology, so a welded mesh rounds off and
a soup of unshared triangles stays faceted. Persisted as
`render.smooth_shading` in state.kdl.

**Show Occluded draws a translucent fill see-through** (`toggle_show_occluded`,
a viewport-menu row under the opacity presets; `render.show_occluded`). In
effect only below full opacity (`State::see_through_active`) — at 100% the
ordinary fill is exact and cheaper, and the toggle says so on the status
line rather than doing nothing silently. The fill then draws through cce-ui's
`SceneDraw::see_through` pipeline (2026-09-25): no face culling, so a closed
mesh shows its far wall, and no depth WRITES, so its near layers hide
neither its far ones. The depth TEST stays on, so what is drawn before the
fill (grid, points, markers) still occludes it.

**The wires go BEFORE a see-through fill, and write depth.** Until
2026-09-25 they drew after it as usual, reasoning that with nothing written
nothing could hide them — which is exactly the bug: every far-side wire
passed and painted OVER the near faces at full strength, so a translucent
sphere read as if its back lattice sat in front of the camera-facing prims.
The wire draw now carries `see_through` too, which selects cce-ui's
depth-writing wire pipeline, and is submitted first; each fill layer then
lands only where it is nearer than the wire under it. A far wire is dimmed
by exactly the layers in front of it, a near wire by none (the fill's
`wire_base_width` offset puts it ahead of its own face), for any mesh
shape, convex or not. The one cost is with a translucent WIRE colour: the
fill layers behind a wire are not drawn under it, so the wire blends over
what is behind the whole mesh rather than over the far fill.
Blending without depth writes is in submission order, so the stage pass
re-sorts the fill's triangles FARTHEST FIRST from the eye
(`geometry::sort_triangles_back_to_front`, centroid distance — painter's
order, exact for non-intersecting triangles and close for the rest) and
re-uploads them whenever the geometry version, the shading or the eye moves
(`State::sorted_fill_key`), which during an orbit is every frame. The eye
is taken in MESH space, the inverse of view × model. Leaving see-through
clears the key and re-uploads nothing: sorted order is still a valid
opaque mesh.

**Every annotation is dimmed by what is in front of it** (since
2026-09-29). Three mechanisms, because there are three kinds of annotation.
The MARKERS (Selected-Group markers, Show Point Markers)
always were: they draw before the fill and the wires and write depth, so a
nearer translucent face or wire blends over them. The LINE annotations —
the normal whiskers, Visualize's vectors, the pull arrows — drew AFTER the
fill until then, which was wrong both ways: behind a translucent fill that
writes depth they were hidden outright, and behind a see-through one, which
writes none, the far side's painted over the near faces at full strength
(the wires' own bug of 2026-09-25). They go before the geometry now, on the
depth-writing line pipeline (`see_through: true` on a wire draw selects
it). The point NUMBERS are 2D text and never meet the depth buffer, so
their dimming is worked out on the CPU: `geometry::point_transmittance`
counts the fill layers the sight line to each point crosses — every
triangle when seen through; otherwise the front-facing ones nearer than all
drawn before them, which is what culling and the depth write leave — and
the label's alpha is `(1 - Opacity)` to that power. Triangles are binned by
screen bounds, so the cost is a projection of the mesh per camera move and
a few tests per label. `State::sync_point_number_alpha` runs it from the
stage pass, for the eye being staged; a label under 2% is not drawn, so
behind an OPAQUE face a number is hidden, where until then every number
showed through everything. The wires are not counted against a number: a
line a pixel wide is not in front of a label in any way one alpha could
show. **A number under a plate is not drawn** (`State::under_a_plate`,
`point_number_labels`, since 2026-09-29). A blur-behind plate samples the
frame so far, which is geometry; the renderer draws ALL text in one pass
after every batch, so a number under a plate stood over it, sharp, where
the markers and wires beside it showed through frosted. Blurring it with
the scene needs a text layer drawn ahead of the plates, which cce-ui does
not have; under the pane tint a blurred 10 px number would not be read in
any case. `a_point_number_under_a_plate_is_not_drawn` is the test.

**What the 2D frame draws over the scene is placed by the camera as it
is when the frame is painted** (`State::refresh_scene_view`, at the top of
`collect_display_list`, since 2026-09-29). The runner paints the 2D frame
and THEN stages the scene, and the stage pass was the one place
`last_scene_mvp`, the pane's rect and the eye were set — so the numbers,
a viewer state's handles and the scale readout were placed by the camera
of the frame before. They trailed the geometry and its markers, which
are meshes drawn by the frame's own matrix, by a frame whenever the
camera moved, and stood a frame's move off their points once it stopped.
`State::scene_view` is the pane and the view as they are now, for both.
The dimming is asked for twice a frame that way and worked out once
(`number_alpha_key`). Not in the traced mode, which keeps the view the
raster pass last staged. `the_point_numbers_are_placed_by_the_camera_as_it_is`
is the test.

**A scene rebuild works the dimming out itself**, for the view the
scene was last staged from (`last_scene_mvp`, `last_scene_eye`). The 2D
frame is painted BEFORE the stage pass, so a rebuild that only cleared
the alphas drew one frame of every number at full strength; a playing
simulation rebuilds at every frame, and the numbers flickered
(`a_scene_rebuild_keeps_the_point_numbers_dimmed`).
`a_point_number_is_dimmed_by_the_fill_in_front_of_it` is the test;
the whiskers' order has none, being a draw list only a renderer reads, and
was checked in a shadow session before and after.

### The Normal node writes point or vertex normals

`normal` has a **Class** row (since 2026-09-29): `Points`, what it always
wrote, or `Vertices`, a normal per CORNER on the detail's vertex store,
with a **Cusp Angle** (shown for Vertices, 0–180, default 60). It is a
class of the one node and not a node of its own, for the reason
`gem_graph` was not ported: two nodes that compute the same thing into two
stores are one node with a choice. A node without the row is one from
before it and writes the points'; the template merge gives a saved
instance the row at `Points`.

`geometry::vertex_normals(geom, cusp)` is the sum, for each corner, of the
face normals around its point that lie within the cusp angle of its OWN
face's — so faces that turn less than the angle from each other are
averaged and read smooth, and those that turn more keep to their own side
and the edge reads hard, which a point's one normal cannot say. At 180 it
is `point_normals` term for term (the same unnormalized cross, so the two
weigh faces alike); at 0 the face's alone.

Two things read a Float3 `N` on the vertices (`own_vertex_normals`, which
also asks that it is as long as the vertices are): the Show Vertex Normals
overlay, and **smooth shading**, which lights each corner by its own
normal where there is one (`smooth_lit_vertices`) and by its point's where
there is not — so a cusp is visible in the fill, not only in the whiskers.
Flat shading is the shader's and reads neither. The attribute rides the
scene's merged `Detail` to both. `the_normal_node_writes_cusped_vertex_normals`
is the test, on a box, whose faces meet at 90 degrees.

### Pull arrows

While the params pane shows an Attribute node that writes `Pos`
(`geometry::moves_points` — the simnet's `pull1` is one), the viewport draws
amber arrows from where points were to where the node puts them. They are
MEASURED, output `P` minus input `P` (`point_displacements`), not read off
Value, so Set, Multiply and an expression all show what actually happened,
and the arrowed points are exactly the ones that moved. An arrow's full length,
head included, is the displacement times **Pull Arrow Scale**
(`State::pull_arrow_scale`, default 1 — the true vector; a dialog row and a
viewport-menu slider, 0.25–10x, persisted in the render block beside Group
Marker Size). The scale is display only: the sampled pairs are kept
unscaled on `pull_arrow_pairs`, and `rebuild_pull_arrow_verts` stretches
each arrow from its fixed base, so a slider drag re-evaluates nothing.

**Strength and Scale By scale the pull** (the Attribute node's Modify,
since 2026-09-29). `Strength` is a `slider` over 0..2 with one in the
middle (a `float` box for its first day), `Scale By` an `attribute`
naming a point attribute whose value weighs each point; the amount that
lands at a point is their product. They scale the EFFECT — the change the
node makes, `old + (combined - old) * amount` — so they mean one thing
under every Combine: an Add moves by that much of Value, a Set goes that
far toward it, a Multiply that far toward the product. Two rows rather
than a longer vector because the Value row is text holding three numbers
and takes no expression per component, while Strength is a number: `$F /
10` ramps a pull in (an expression is not clamped to the slider's range;
a value set on the slider or typed into its readout is), and the trackball keeps the direction while one
slider sets how hard. At an amount of exactly one the combined value is
written as it always was, bit for bit, so a save from before the rows
(which the template merge gives a Strength of 1) solves to the same
numbers. A Scale By naming no attribute is an error on the node and moves
nothing. The arrows are measured, so they show the scaled pull.

**Composite broadcasts a single number** (since 2026-09-29): a Source B
of ONE component is every component's, so a Float3 times a Float is the
vector scaled — by a constant, or per point by a weight. The componentwise
operations used to pair the number with X and zero with the rest, which
kept X and zeroed Y and Z under Multiply and touched X alone under Add. A
Source B of two or more components still pairs off by position; Dot,
Distance and Length reduce to one number and did not change.

**Per Frame makes the amount a rate.** Inside a simnet the chain runs once
per SUBSTEP, so a pull that lands whole each run pulls four times as far
a frame at four substeps — the substep count, which is there to steady a
solve, becomes its speed. With `Per Frame` on (the template's default)
the node reads the solver's `dt` off the state and scales what
ACCUMULATES: an Add lands `dt` of its amount, a Multiply the `dt`-th
power of its factor, which is what compounds back to the factor over a
frame. A Set does not accumulate and is left alone, and the row is shown
only for the other two. Outside a simnet there is no `dt`. It is a
switch rather than the rule because the other use of an Add is real: a
chain that adds one to a counter to COUNT its runs
(`test_substeps_run_the_chain_more_than_once_per_frame`) wants the step,
not the frame. A node that does not carry the row reads off, which is
every hand-built node; a saved instance gets the row, on, from the
template merge — so an existing pull in a simnet with substeps above one
moves a substep-count slower after this, which is the point.

At most `State::PULL_ARROWS_MAX` (12) points get one, picked by farthest-point
sampling (`spread_sample`) so they cover the region the pull covers rather
than bunching wherever the point numbering runs locally. A node inside a
simnet is measured as this frame's last substep saw it, with that substep's
feedback pushed, the same rule the dived-in scene walk draws by: an arrow
runs from where the point went INTO the pass that produced the displayed
state, so its tip lands on the displayed point whenever the pull is the
chain's last mover. Without the feedback the `input` node reads the seed,
and the arrows would sit where the points started, not where they are. `sync_pull_arrows` borrows the shared sim cache for
this, so during playback the feedback is a cache hit rather than a re-solve
from the seed every frame; it runs from both `sync_nodes` and the end of
`rebuild_scene_geometry`, keyed by (node id, params, geometry version).

### Dragging the scene orbits the camera

`State::orbit_camera_by` turns the camera by a drag delta, armed by a left
press that `cursor_in_viewport` says landed on scene. Before it the camera had
NO drag gesture at all: `Viewport3D` handles only `MouseWheel`, so the scene
turned by scrolling and by nothing else — which suits a trackpad and leaves a
mouse with no way to look around.

`ORBIT_RADIANS_PER_PX` is the trackpad's own pixel-delta constant, so a drag
and a two-finger swipe turn the scene at the same rate rather than feeling like
two different cameras. The default camera carries its orbit in
`rotation_x`/`rotation_y`; a NAMED camera accumulates into
`pending_yaw`/`pending_pitch` for its node to pick up — the same split the
scroll path makes, so a dragged camera and a scrolled one mean the same thing.
A drag stops when the pointer does (`reset_velocity`), unlike a flicked scroll,
which coasts.

Precedence matters and is load-bearing. The press arms AFTER the viewer state's
own press hook, so dragging a curve handle still edits it, and after the node
hit tests, so a press on a node still moves the node. "Empty" means the scene
really is what is under the cursor — which, with the network overlaying the
window, is exactly what `in_network_pane`'s node test decides.

**The camera PANS as well** (`State::pan_camera_by`, since 2026-09-29;
until then the view only turned and zoomed about its pivot, and the
pivot moved by Frame All alone). A pan slides the pivot and the eye
together across the plane of the screen, so the view turns nowhere and
what is on the PIVOT's plane follows the pointer px for px — the
projection is a perspective, so what is nearer moves further. Three
gestures, `pan_drag` beside `orbit_drag`:

- **the middle button, dragged** over the scene. Armed after the
  network's own pan, which the middle button is wherever the network is
  laid out — with its plate off that is the whole window — and AHEAD of
  the press cascade's `button != Left && != Right` return, which is where
  the first cut put it and where it never ran;
- **shift and the left button**, in the orbit's own arm, so a viewer
  state's handle and a node still take the press first;
- **shift and a scroll**, at the top of the wheel arm, a notch being
  `PAN_PX_PER_LINE` px.

The Default Camera's eye hangs off its pivot, so `Viewport3D::pivot` is
all that moves. A camera node has its Pivot and Position rewritten, to
four decimals; a pan is hundreds of small moves and each read back from
the text would lose what the text could not hold, so `State::pan_exact`
keeps what the last pan wrote in full and is used while the node still
says it. `the_camera_pans_with_the_pointer` is the test.

**The active camera's name lives in two places, and `State::set_active_camera`
is the only writer of either.** The viewport widget keeps its own copy because
its wheel handler routes by it — the Default Camera's orbit lands on the
widget's `rotation_x`/`rotation_y`, a camera node's accumulates into
`pending_yaw`/`pending_pitch` for `tick_frame` to write onto the node — and
until 2026-09-24 that copy was written once, at construction, from whichever
project `State::new` loaded. Open a project whose active camera differed (the
startup default-project pointer, Open, New, the viewport menu) and the two
disagreed: the widget parked every wheel into the pending pair, the drain saw
the Default Camera active and discarded it, and trackpad scrolling in the
viewport did nothing while a drag — which reads `State`'s copy — still orbited.
`a_wheel_orbits_the_camera_that_is_active_after_a_change` covers the three
paths.

### Commands, chords and the palette

`src/command.rs` is one list of everything the app can be asked to do. Each row
carries its `id` (snake_case — this is what `input.kdl` binds, so it follows
that file's existing convention and must not change when the label does), its
`label` (what the palette and menus show), a `Context` (which pane it belongs
to), a `Run` (how it reaches the work), and a `default_chord`.

Before it there were three vocabularies with nothing holding them together: the
`Action` enum matched against chords, the menu-item LABELS `execute_menu_action`
dispatches on, and a hand-written registration block listing which `Action` got
which chord. A command lived in whichever of them someone had needed, and
nothing could tell you which ones had no binding at all. `ShortcutManager` now
binds **command ids**, not `Action`s — which is also what lets a chord reach a
menu-dispatched command like Open, something no binding could do before.

`Run` has two variants because the app genuinely has two dispatch paths; a
command names exactly one, so the palette, the chord and the menu all end up in
the same code. `State::run_command(id)` is the single entry point, and it is
exposed over MCP as `run_command` — every command is scriptable, including the
ones no menu label reaches.

**The toolkit's runner claims four chords before the app sees them**, from
`input.kdl`'s `cce-ui` domain: `undo` (ctrl+z), `redo` (ctrl+shift+z),
`focus_next_group` (ctrl+tab) and `focus_prev_group` (ctrl+shift+tab). Undo and
Redo are therefore registry rows with NO default chord — not an oversight: the
runner routes them to the focused widget first, so a text box undoes its own
typing before the app is asked, and registering ctrl+z here would quietly take
that away. Focus Next/Previous Pane do override the runner's group chords, which
is deliberate and predates the registry. `command::conflicts` cannot see any of
this — it compares this app's bindings with each other — so it is written down
here instead.

`conflicts()` reports two commands resolving to one chord at startup, because
the failure is otherwise silent and looks like a broken command rather than a
broken binding: `match_command` returns the first match and the second simply
never runs. It compares chords as PARSED, not as text. That exposed a real bug:
`Shortcut`'s derived `PartialEq` compared character keys byte for byte while
`matches()` compared them case-insensitively, so `Ctrl+S` and `Ctrl+s` were one
keypress at the keyboard and two distinct values in memory — and the collision
detector quietly failed to report exactly the collision it exists to catch. Both
now go through one `same_key`.

**The palette** is the dialog's Commands half (`src/dialog.rs`, below), not a
widget of its own. Ranking is `fuzzy_rank`, which reproduces the plugin's
fuzzyfinder exactly — shortest contiguous span, then earliest start, then
alphabetical — so muscle memory survives; the focused pane's commands are then
partitioned to the front, stably, without dropping anything (a palette that
hides what you are looking for is worse than one that lists it second). Each row
carries its chord in a column of its own, so the palette teaches the keyboard
rather than replacing it.

It was a `cce-cloud --dmenu` popup until 2026-09-19: a second PROCESS with its
own window, handed one line of text per row on stdin and answering with one line
on stdout. Everything awkward about it followed from that pipe — the chord had
to be padded into the label to fake a column (a tab rendered as one literal
stop, so they came out ragged), and the answer had to be matched back to a
command by the LONGEST label the row starts with, since "Save" is a prefix of
"Save As"'s row. `palette_row` / `from_palette_row` were that encode/decode pair
and are gone with it; `fuzzy_rank` and `palette_entries` survive, because the
ranking was never the problem.

`command_palette` (Ctrl+P) and `toggle_dialog` (Alt+D) both reach the same
dialog and differ in exactly one way, which is the reason both rows exist:
Ctrl+P OPENS it (with a fresh query, never closing), Alt+D toggles it.

`test_every_menu_command_names_a_label_that_is_dispatched` scans `app.rs` for
`execute_menu_action`'s arms. Scanning source is an odd way to assert it, but
the alternative is calling every command to see whether it is handled, and
"Exit" would end the test run. It is the check the plugin's `hccommands.py` doc
argues for: a label kept in two places drifts, and a renamed one fails silently
— the dispatch falls through its match and the command does nothing.

### The dialog (Alt+D, Ctrl+P, Tab)

`src/dialog.rs` is the app's one modal overlay, and **every filterable list
in the designer is an opening of it**. It is one roster slot, `DIALOG_IDX`,
an app-owned `Dialog` that paints the plate, the query line and the row
list — and the rows' controls, from the toolkit's own stamps (`Toggle`,
`Slider`) and hosted `ColorSelector`s, so a slider in the dialog is the same
slider as a slider in the params pane.

`Mode` says what an opening is for, and it is the reason there is one widget
rather than two:

- `Mode::Commands` (**Alt+D**, **Ctrl+P**) — ONE list: every registry
  command, fuzzy-filtered in place, and every display setting
  `DesignSettings` persists, ranked among them. Until 2026-09-24 the
  settings were a second HALF behind a tab strip, a second `ParametersBg`
  slot (`DIALOG_PARAMS_IDX`) laid out inside the plate with section headers
  and no filter. A setting is something you ask for by name exactly as a
  command is, so it ranks in the same list; the strip, its two labels, the
  section rows and the second slot are gone, and with them the double
  paint, the `dispatch_uncovered` routing into a second slot and the
  `dialog_settings_shown` baseline the writeback diffed against.
- `Mode::Rename` (the node menu's **Rename**, the `rename_node` command)
  — the query line is the NAME, opened holding the one the node has, and
  the one row (`RENAME_ROW_ID`) says what Enter will do: `Rename camera1
  to lens`, the name as it will be written (`sanitize_node_name`, so
  `My Ball` reads `my_ball` before it is committed), or why it will not
  be — its name already, another node's, none. `State::rename_check` is
  that rule and `State::rename_node` the one entry the dialog and MCP's
  `rename_node` share; a sibling's name is refused in both, since wires
  are by name. `State::rename_target` holds the node by id.
  `a_node_is_renamed_from_its_menu` is the test.
- `Mode::Groups` (the `group_markers` command, **Group Markers** in the
  palette — the palette TRANSFORMS into this list, since 2026-09-29) —
  the scene's point groups, one row each with a switch and the member
  count in the chord column, filtered by name. A row's switch marks the
  group's members in the viewport: a sphere at Group Marker Size in the
  selected group's amber, on every member, staying on whatever is
  selected. Enter or a click flips it in place and the list stays up, as
  the palette's toggles do. `State::marked_groups` is the set, persisted
  in the viewport block (one comma-joined string — a KDL list of one
  reads back as a bare string, which a `Vec` refuses, and a settings file
  that fails to parse reads as the defaults), so it rides the project
  file too. `State::scene_groups` is every point group of the scene as
  last built with its members' positions, kept by `rebuild_scene_geometry`
  so the list and the markers (`rebuild_marked_group_verts`,
  `meshes.marked_points`) come from what is on screen and a switch
  evaluates nothing; the markers follow the geometry through a rebuild.
  A marked name the scene has no group for marks nothing and is kept, so
  a group that comes and goes with a frame does not lose its switch.
  `the_group_markers_dialog_marks_a_groups_points` is the test.
- `Mode::AddNode` (**Tab**, in the network pane) — one list of node
  templates, and a pick that instantiates at the grid cursor. Tab is what
  opened it, so Tab closes it again. The query hint names the mode; there
  is no title band, so the two openings are the same plate.

Both modes share the plate, the keys and `fuzzy_rank`, which is the whole
point — the app used to put two filterable lists in front of the user that
looked and behaved nothing alike.

**A row's control is `Row::control`, an `Option<Control>`**, and a row that
has one is worked IN PLACE — the dialog stays up, the control re-reads, the
selection stays where it was:

- `Toggle` — a toggle command's switch (`command_toggle_state` is the
  table, the same read the View menu's checkmarks are set from), painted as
  the toolkit's `Toggle` in a right-hand column reserved for every row as
  soon as any row has one, so the chord column keeps a straight edge. Enter
  or a click flips it. `dialog_toggle_rows_cover_every_toggle_command` fails
  when a `toggle_*` / `show_*_pane` command is added without an arm in the
  table, because the miss is silent — the row just ships plain.
- `Slider` — a value over a range, to `dec` decimals; `dec` 0 snaps to
  whole numbers, which is the spinbox shape (Grid Thickness in thousandths,
  Origin Size in tenths — the units those params always used). One toolkit
  `Slider` stamp in a `RefCell` serves every slider row, set to each row's
  range and value as it is painted. The band **begins `SLIDER_W` in from the
  row's right end and runs out to the CHORD column's right edge**, so it
  ends where every other row's key binding ends and the switch column stays
  clear; the readout sits AHEAD of the band, and a press tests the band
  alone — over the whole control a click on the readout would jump the
  value to whichever end of the range it abuts. A press jumps to the
  pointer and arms the app's widget-drag protocol on `DIALOG_IDX`
  (`Dialog::draggable` / `drag_*`), so the value follows the pointer off the
  plate; the wheel over the control turns it (2% of the range a notch) where
  over the rest of the list it scrolls; Left/Right nudge it by the row's
  `step` while it is selected; Enter on it runs nothing. **A slider row
  lands through `State::land_draw_time_setting`** — the one landing the
  viewport menu's sliders use, by `DesignSettings` field key: the field,
  the one marker mesh it feeds, a redraw, then `refresh_dialog_controls`
  — NOT `apply_setting`, and state.kdl is written once on the drag's
  release (`dialog_mouse_input`), or at once for a wheel notch or arrow
  key. Until 2026-09-28 every motion of a drag ran the full apply: a graph
  evaluation, two more keyed on the version it bumped (group markers, the
  params pane's pickers), a path-tracer restart and a synchronous file
  write, per pointer event, for six values the graph never reads — which
  is what made the dialog's sliders drag behind the pointer while the
  menu's did not. The spin rows (Grid Thickness, Origin Size, Camera
  Pivot Size) land the same way, their whole number
  over the row's unit, each re-baking only the guide mesh that reads it;
  `a_dialog_slider_drag_lands_without_re_evaluating_the_graph` pins all
  of it. The **zoom row**
  (`ZOOM_ROW_ID`, only while the network pane is focused, since zoom is that
  pane's) is one of these over `State::zoom_percent` (100 = Reset Zoom,
  range the pitch limits), landing through `set_zoom_percent`, which zooms
  about the cursor cell and re-reads the row, since `zoom` clamps.
- `Choice` — a fixed set (World Unit). The current option reads in the
  chord column between two arrows; Enter or a click steps to the next,
  Left/Right either way.
- `Color` — a hex colour. Behind each colour row the
  dialog keeps one toolkit `ColorSelector` (`Dialog::colors`, by row id,
  kept across re-rankings so a query that drops the row does not kill its
  picker): a real widget, not a stamp, because it carries state — a hex
  edit in progress, a `cce-color-editor` process streaming values. It is
  painted over the band and handed presses on the band with the band as its
  rect (`color_event`); `Dialog::tick` polls it; a change comes out of
  `take_color_changes`. **While its hex well is being typed into it has the
  keyboard ahead of everything** — `dialog_key_input` forwards to
  `editing_color` first, so Escape and Enter end the edit rather than the
  dialog.

**A setting row edits the live field** — see "There are no meta nodes"
above, which is where these values used to live and why a direct write did
not stick. `SETTINGS` is the table of which row belongs to which owner:
`Owner::Field` (a live field, with a `Ctl` saying what control draws it,
since a bare Rust field carries no type or range the way a param did) or
`Owner::ActiveCamera` (an active-camera param with the live field as its
fallback — the Default Camera has no node). The toggles the retired
subnets held are NOT rows of the table: each is a registry command with a
switch on its own row, and a second row per toggle would have listed every
switch twice. A row's id is its label under `SETTING_ROW_PREFIX`
(`setting_of_row` resolves it back), its control is built by
`setting_control` from the value `setting_value` reads (the params pane's
encodings — a hex, a whole number in the spin's unit, an option's text),
and every change lands through `apply_setting(label, value)`: write to the
owner, then the one regenerate-and-persist pass (the viewport meshes bake
their sizes and colours in) and `refresh_dialog_controls`, which re-reads
every control in place — not `refresh_dialog_rows`, which re-ranks and
would throw the selection to the top. `dialog_settings_rows_name_owners_that_exist`
is the backstop, because the failure is silent — a `Field` key no dispatch
arm names reads a default and writes nowhere, so the row draws, takes an
edit and does nothing, which is why that test round-trips every one of them.
**Group Marker Size** is the one row added with the collapse: the
Selected-Group markers' radius in world units
(`State::group_marker_size`, persisted in the render block; it was a
multiple of the retired Point Size until 2026-09-29).

**The open project's PATH heads the Commands list**, as a row rather than a
command (`PATH_ROW_ID`): the label is the path, the chord column carries the
file name — the palette's readout of what is being edited, in the column a
command's chord would use — and picking it copies the path to the clipboard
and closes, a copy being done the moment it happens. It ranks against the
path text like any other row, so a query finds or drops it.

Two details. The label truncates on the LEFT (`Row::truncate_head`, the
paint's `fit_head`), because the tail of a path is what identifies it and a
row cut down to `/home/me/pro...` would name every project in the directory
equally badly. And there is NO row when no project is loaded: the bundled
`default_project.json` leaves `loaded_project_path` None on purpose (the
window title and Set As Default take the same position), and a row offering
to copy a path into a versioned file in the source tree would be a trap.
`project_path_readout` reads the name with `file_name()`, the same call the
window title makes, so the two cannot disagree about what is open.

The actual clipboard write is `#[cfg(not(test))]`. `wl-copy` has to OUTLIVE
its caller to serve the selection, and it inherits the test binary's captured
stdout — so a test that really copied left cargo waiting on a pipe held open
by a clipboard daemon, which looks exactly like a hung suite.

**Alt+D, not Super+D.** Every Super chord is the compositor's before any client
sees one (`input.kdl`'s `cce-window-manager` domain has `super+d` on the app
launcher), and Super held is the DE's window-adjust modifier besides. Alt is the
app's own — the `move_*` family already lives there.

**The dialog plate IS the menu plate** (since 2026-09-28). The render arm
draws it with `cce_ui::widget::context_menu::paint_menu_plate`, the one
function the context menus draw theirs with: `Material::menu` — the
`style.surface.menu` block's `color` (a cce-ui key added the same day;
absent, the root plate colour as menus always wore), `opacity` and
`compression` — on `menu.corner_radius` with the relief-width roll. So
the command palette, the Add Node list and every right-click menu are
configured in ONE block and cannot be configured apart; the shared
config.kdl's `menu color=(rgba)"#101018ff"` sets that block to the look the
dialog had (there is no per-app `cce-designer/config.kdl` on the machine as
of 2026-09-28 — only `.bak` copies — so the main file is where it is set).
Until then the dialog
was the parameter plate's fill under a compression of its own
(`DIALOG_COMPRESSION` 0.8, `style.surface.dialog.compression`) — an
in-app override the menus did not share, and the reason the two plates
looked nothing alike. `the_dialog_plate_is_the_menu_plate` scans the
source for that override coming back. The one difference left is
mechanical: the menus are hosted in the runner's popup surface, where the
compositor's blur cannot compress and the helper folds `compression` into
opacity, while the dialog is in-window and the in-app frost pass
compresses as configured.

**The dialog is painted after the overlay passes, not in the widget walk.** A
high `z_order` is not enough: `append_frame_text`, `append_scale_readout` and the
point-number overlay all run AFTER the whole walk, so the graph's node labels drew
straight over a dialog that had already covered them. `append_dialog` runs
between the plate corners and the context menu instead.

**And even that is not enough, because text is not painted in display-list
order.** The engine collects every `Prim::Text` and lays them all out at the end,
so a plate over a label does not hide it at any depth. What hides it is the
engine's popover-occlusion clamp, which reads `UiContext::active_popovers` — so
`Dialog::popover` claims the dialog's whole rect, and the designer's
registration loop picks it up. The clamp exempts text whose own bounds COINCIDE
with the occluder, so every label inside the dialog carries the dialog's rect and
truncates itself; a hosted colour selector is painted twice for this (once
for its well and swatch, once into a scratch `PaintCtx` whose text alone is
re-emitted retagged), since a `PaintCtx` can be handed text back but not
geometry.

**`Dialog::occluding` exists because that one claim serves two mechanisms that
want opposite answers.** `UiContext::is_coordinate_covered` reads the same
`popover_rect` — off every REGISTERED widget, not just the ones in
`active_popovers` — to decide a press landed under something else. With the claim
standing, every control inside the plate is covered by the plate it is drawn
on and nothing can be clicked; the toggles looked laid out, painted, and
completely inert. `State::dispatch_uncovered` lowers the flag for the length of a
dispatch into the dialog and puts it back, invalidating the coverage memo on both
edges (the engine queries it on every left press, so lowering the flag alone
leaves a stale cached answer).

**A control in the dialog lifts under the pointer** (`Dialog::hover_ctl`,
since 2026-09-29): the row whose switch, slider (readout lane included)
or colour well the pointer is over, found by `control_rect` on every
move. The toggle and slider stamps serve every row, so each is told the
hover as its row is painted (`Toggle::set_hovered`, `Slider::set_hovered`
— the two setters cce-ui grew for it, since a stamp is never routed a
`MouseEnter`); a colour row's selector is a widget of its own and is
told by `MouseEnter` / `MouseLeave` through `color_event` as the pointer
crosses its band. The hover clears with the row's when the pointer
leaves the plate, since `broadcast_pointer` hands the dialog an
off-screen position then. `a_palette_control_lifts_under_the_pointer`
is the test.

**A captured pointer hovers no pane.** `State::broadcast_pointer` hands
every slot the pointer's position, or an off-screen one while
`pointer_captured` says a gesture or the dialog owns it — a widget or app
drag, an orbit, a pan, a grid expansion, a handle grab, a held menu
slider. It runs from the cursor arm (ahead of the early returns those
gestures take, which is where a pane hovered at the press used to stay lit
for the whole drag), from the release once the captures are down, and
from the dialog's open and close, since a modal that let the panes beside
its plate keep hovering was a modal in name.
`a_captured_pointer_hovers_no_pane_and_the_release_hands_it_back` is the
test.

Input is intercepted whole, at the top of `handle_event`'s keyboard and mouse
branches: `dialog_key_input` is TOTAL rather than a layer, because the network
pane's bare-letter family is ungated and typing "frame" into the filter would
otherwise step the grid cursor four times and flip a node's geometry toggle on
the way past. A press outside the plate dismisses and is swallowed, the way the
node and plate-corner menus behave.

**Nothing in this crate shells out to `cce-cloud` any more**, and
`nothing_shells_out_to_cce_cloud_any_more` scans the source to keep it that
way. Retiring the two popups took a surprising amount of scaffolding with
them: `CloudPopupTracker` (the single-active-popup toggle bookkeeping), the
`CloudSpawned` / `CloudClosed` events that adopted a popup's pid, the
`RunCommand(&'static str)` event that existed because the popup ran on its own
thread and could not touch `State`, and the `libc` dependency, whose only use
was `kill`ing a stray popup. `active_menu_cloud_pid` / `_idx` and the
`menu_closed` MCP tool went too — they were already dead, left from a retired
attempt at menubar dropdowns over `cce-cloud`, and nothing had set them to
`Some` in a long time. `cce_ui::process::CloudPopup` outlived that by nine
days as unused public API in a shared crate — the designer had been its only
consumer — and went on 2026-09-28 along with the whole `process` module and
cce-ui's `tokio` dependency, which existed for nothing else.

### Runtime paths point into the source tree

Node templates (`nodes/*.json`) and `default_project.json` are located via
`env!("CARGO_MANIFEST_DIR")` — the installed binary still reads from the source
checkout. Templates are resolved recursively: a template's children reference other
templates by `type`, merged with param overrides (`load_fs_tree` in `src/app.rs`).
Missing referenced templates panic at load.

Saved instances are self-contained copies, but the loader merges template
evolution into them (`merge_template_defs` in `src/app.rs`, run on every
project deserialization including thumbnails): missing params are inserted
where the template puts them (after the last template param the instance
already has — so the Sphere's Method lands above Radius in an old save, not
below Color), existing ones keep their value but take the template's UI
metadata, and a subnet template (the Embryo, since the four kernel subnets
went native) refreshes its children's params and any child's `Code`
outright — **the template owns the surface and implementation, the
instance owns its values.** A script hand-edited inside a template
instance reverts on load; custom scripts belong in bare wrangle nodes,
which the merge never touches.
Native nodes match their template by type, subnet instances by name
("sphere3" → "Sphere", case-insensitively) plus a full child name/type match; the merge never
injects or deletes children and never rewrites files on disk.

### Node names are lowercase and carry no whitespace

A node's name is a segment of its path — `/sphere1/opencl1` is how the
breadcrumb, the MCP tools and every `Input` wire name it — so names are
lowercase, as Houdini's are, and carry no spaces (since 2026-09-21).
`sanitize_node_name` (src/app.rs) is the rule: the conventional space
between a template name and its index goes ("Sphere 1" → "sphere1", which
is also what minting now produces), any other whitespace becomes an
underscore ("My Region" → "my_region"), the whole thing is lowercased, and
empty comes back as `node`, because a path convention with exceptions is two
conventions. The template merge matches an instance to its template
case-insensitively ("sphere3" → "Sphere"). It runs at every entry point — minting, the
`add_node` name override, `rename_node` — and as a LOAD-TIME MIGRATION on
every load path, `Project::sanitize_node_names`, called before the template
merge in all five places a project is deserialized (the two `load_from_file`
branches, `State::new`, the thumbnail and the export CLI).

The migration follows references, because wires are by name: within each
level it renames the children, then rewrites any sibling parameter whose
value was one of the old names (`Input`, `With`, `Rest`, `Target`, `Source`,
`Collider` — any of them, since it matches values rather than a list), and
maps the view state's active camera, the one reference outside the tree. A
sanitized name that lands on a sibling's ("Sphere 1" beside a hand-named
"sphere1") steps aside with a `_2` suffix rather than leaving two nodes one
name and every wire to them ambiguous.

## Repo hygiene

`scratch/` holds ad-hoc debug scripts/logs and `screenshot*.png` at the root are
debugging artifacts — not source, don't extend them. Tests live in
`src/main.rs`'s `#[cfg(test)]` module; add new ones there. The exception is
`tests/`, which holds the two tests that SCAN the crate's own source —
`doc_claims.rs` (CLAUDE.md's `(~Nk lines)` figures) and `user_paths.rs` (below)
— and they are out there because a scanner under `src/` is the first thing it
finds. Both are deliberately mirrored per crate rather than shared, since every
crate here is its own git repository that must build standalone; they need
nothing but `std`, so copying one into a sibling is the whole job. Commit
messages follow `feat:` / `fix:` / `refactor:` style (see `git log`).

**`tests/user_paths.rs` refuses source that builds a path the user owns**, the
class of bug that had this suite rewriting `~/.config/cce/cce-designer/state.kdl`
on every run (see "App-written settings" above). Two rules, one per shape that
actually shipped: no line assembles a config path out of `.config` by hand —
`cce_ui::config::cce_config_dir()` is the only way in, because that is what the
`cfg(test)` redirect keys off — and every `temp_dir()` is scoped with
`std::process::id()` within a line or two, since /tmp is one namespace shared
with every other user and every concurrent run. Verified by reintroducing each
bug: both are caught, naming the file and line. The `temp_dir()` rule is not
limited to tests, because a fixed /tmp name is no better in shipped code.

Two runtime guards sit alongside it in `src/main.rs`, since a scan cannot see
behaviour: `the_suite_does_not_write_the_users_own_settings` and
`the_recent_files_list_is_not_the_users` each snapshot the real file, exercise
the write path, and assert it did not move — the second checking the load side
too, because reading the user's recent list would make the suite's behaviour
depend on the machine.
