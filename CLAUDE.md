# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-designer` is a node-based procedural 3D design app (Houdini-style) for the cce
desktop environment: a node graph is evaluated into geometry — OpenCL kernels do the
generation — and displayed in a 3D viewport with both a raster pass and a path-traced
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

Some tests (e.g. `test_sphere_subnet_geometry_generation`) execute real OpenCL
kernels THROUGH the OpenCL runtime when one exists; without one they exercise
the CPU reference backend via the automatic fallback, so the suite is green
headless. `kernel_cpu`'s template tests always run the CPU side; the
cross-validation test compares backends and skips silently with no platform.

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
  Note that detaching REWRITES `default_project.json` in the source tree, since that
  file is the sync channel; it is versioned, so check `git status` after testing.

### MCP automation server

The main window runs an embedded MCP server on `127.0.0.1:3001`
(`CCE_DESIGNER_MCP_PORT` overrides so a second instance can run alongside;
`src/api.rs`). This is the way to drive/inspect the running app: attach with
`claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`, or
speak JSON-RPC directly with curl (`initialize` / `tools/list` / `tools/call`).
There is one tool per `McpAction` variant (tool name = the variant's serde
tag, dispatched in `apply_mcp_call` in `src/window.rs`) plus `get_state`. The
tool list lives in `mcp_tools()` in `src/api.rs`; the protocol layer is
`cce_ui::mcp` (tools-only Streamable HTTP). Keep the enum, the tool list, and
the schemas in sync — `test_mcp_tools_map_to_actions` enforces the mapping.
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

- `src/app.rs` (~8.9k lines) — the heart: `State` (the entire app model), `McpAction` /
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
- `src/kernel_cpu.rs` — the CPU reference backend for node kernels: a
  tree-walking interpreter for the C subset the kernels use (scalars, arrays,
  user functions with pointer params, casts, the positional buffer ABI),
  running the exact launcher contract of `run_opencl_kernel_with_params`. It is
  the SEMANTIC REFERENCE — `cpu_matches_opencl_on_every_shipped_kernel`
  compares the two backends vertex-by-vertex on every template kernel when a
  platform exists, and the absolute template tests keep kernel coverage green
  headless. Selected automatically when there is no OpenCL platform (one
  stderr note), or forced with `CCE_KERNEL_CPU=1`. A kernel that fails ON a
  present platform does not fall back — the error is in the kernel, and the
  GPU diagnostics should surface. Step-budgeted so a non-terminating kernel is
  an error, not a UI freeze. No vector types / barriers / local memory.
- `src/geometry.rs` — node-graph evaluation. Every evaluator threads an
  `EvalSim` (current frame + `SimCache` + feedback stack) alongside the error
  slot. The `simnet` node type iterates: the chain between its `input` and
  `output` children is one simulation STEP; step 1 eats the simnet's own
  `Input` (like a subnet), each later step eats the previous state, which the
  `input` node reads off the feedback stack instead of jumping to the outer
  graph. Solves run up to the playbar frame and cache per node id on `State::
  sim_cache` (playing forward = one step per frame); the cache key hashes the
  simnet subtree + seed, so edits restart the sim, and backward scrubs restart
  from the seed (steps are not invertible). The scene walk does NOT recurse
  into a simnet's children — that would draw one un-iterated pass of the chain
  on top of the solved result. Dived INTO a simnet, the output child's
  geometry flag draws the solved state, and every OTHER visible child draws
  itself as the current frame's step saw it, with the feedback stack holding
  the state that step consumed (`simnet_step_feedback`, read off the
  `SimSolve` the solve already keeps): `input` shows what the step reads, a
  chain node shows this frame's pass, and a node not wired into the chain at
  all simply draws. Until 2026-09-21 only the output flag drew, and a visible
  node inside a simnet was a node you could not see. At any
  displayed level `input`/`output` children draw their resolved geometry (top
  level of the walk only, so outer views don't draw subnet chains twice).
  Frame changes invalidate the scene only when the
  graph `contains_simnet`. Each OpenCL node's kernel code is
  preprocessed: `chf("name", default)` / `chi` / `chv` calls are parsed into dynamic
  UI parameters (`parse_dynamic_params`) and rewritten to `param_values[i]` reads
  (`preprocess_opencl_code`). `network_sphere_vertices_with_errors` walks the graph
  from output nodes; OpenCL failures are collected, not fatal.
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

  Two implementations ship, deliberately different in shape, because an
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

  The HUD draws one line ABOVE the scale readout, sharing its left margin — not
  at the top, because the viewport is full-bleed and the pane plates float over
  its top edge, so a mode line there lands under the collapsed stubs. It exists
  because a viewer state changes what every click does and snapping silently
  changes what a drag does.
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
them is the one rule worth remembering: **a display setting belongs to the
view, so it is a live field on `State`, persisted to `state.kdl`, and
reached from the command palette.** Never a node.

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
  and edited in the dialog's **Settings** half — `SETTINGS` in
  `src/dialog.rs`, whose rows are `Owner::Field` (a live field, with a `Ctl`
  saying what control draws it), `Owner::Command` (a registry toggle, read
  through `command_toggle_state`), or `Owner::ActiveCamera`. The table is now
  the app's whole display configuration, so a value left out of it is GONE,
  not merely hidden — `every_retired_subnet_setting_is_reachable` is the
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
  `toggle_render_points`.
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

`DesignSettings` (viewport/graph display state the app rewrites itself:
colors, grid sizes, show flags) persists to `state.kdl` — deliberately NOT
`config.kdl`, which is the user-authored toolkit-config override slot that
cce-ui auto-merges (see `../cce-compositor/WORKSPACE.md`). Legacy `design.kdl` / `design.json`
files migrate on load. Scroll behavior (`scroll_speed`, `inertial_scroll`,
`scroll_friction`) is intentionally absent: it is config-owned
(`input.inertial` in config.kdl) and must not be shadowed by app state.

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
seven real `~/.config/cce/cce_designer-<hash>/` directories behind.

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

### The OpenCL ICD corrupts unrelated file descriptors

**Mesa's Rusticl ICD closes a file descriptor it does not own**, somewhere
under `clGetPlatformIDs`. Caught under `strace -k` on 2026-09-23, the stack
reading `__close` <- `libRusticlOpenCL.so` <- `clIcdGetPlatformIDsKHR` <-
`clGetPlatformIDs`. By the time it closes, that descriptor number has been
recycled to whatever another thread opened a moment earlier, so that thread's
next `read` comes back **EBADF on a file nothing is wrong with**.

It surfaced as a flake with no apparent connection to any of this: roughly one
`cargo test` run in eight failed somewhere unrelated, most often
`test_every_template_names_a_type_something_resolves` reporting "load_fs_tree
returned 50 of 51 templates". The victim is whichever file lost the race —
`sphere.json`, `output.json`, `hull.json`, a different one each time — and the
tests that then failed were simply the ones that needed it.

Three things follow, and the order they were tried in is worth keeping,
because two of the three plausible fixes did nothing:

- **Probing less often does NOT help.** Four tests each called
  `get_platforms()` to decide whether to skip; caching the answer
  (`has_opencl_platform`) and serializing every enumeration behind one mutex
  (`probe_platforms`) took a process from dozens of overlapping enumerations
  to two that cannot overlap — and the failure rate did not move (9 in 80,
  against 10 in 60 before). The dangerous window is opening the ICD at all,
  once per process, not how many times we ask afterwards. Both are kept
  anyway: they are right on their own terms and cost nothing.
- **Forcing the CPU backend did not help either, until it actually meant it.**
  `CCE_KERNEL_CPU=1` selected the CPU kernel path but everything still
  *probed*, and `cpu_matches_opencl_on_every_shipped_kernel` called straight
  into `run_opencl_kernel_with_params`. That function now refuses at the top
  when the backend is forced — one gate, reported as the no-platform error
  every caller already handles — so forced-CPU never loads the ICD.
- **What actually works is not loading the ICD.** `CCE_KERNEL_CPU=1 cargo
  test` is the reliable way to run the suite, and `OCL_ICD_VENDORS=<empty
  dir>` is the sharper instrument for confirming the ICD is the cause: with no
  vendor to load, the EBADF failures disappear outright.

The bug is in the ICD and cannot be fixed from here. What can be fixed is
never being silent about the damage: `load_fs_tree` used to drop a template it
could not read or parse inside an `if let Ok` pair, so the node just left the
palette — which looks nothing like an I/O error and nothing like a parse error
either. It says which file and why now, on stderr. That one change is what
turned an afternoon of bisecting into a diagnosis.

The app is far less exposed than the suite: it reads its templates at startup,
on one thread, before OpenCL is in play. The suite is exposed because libtest
runs its tests in parallel.

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
and paste still pastes the right path. The paste writes `chs()` when the
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

### Sibling-first inputs and the Switch node

Two pieces added on 2026-09-21 so a node can be BUILT FROM other nodes
the way a Houdini HDA is — the Embryo is the first to be recomposed that
way — both in `src/geometry.rs`:

- **`find_input_node(root, target, name)` looks for a SIBLING first, then
  anywhere.** Every resolver used to search the whole tree from the top, so
  inside the second instance of a subnet a child wired to "input1" found the
  first instance's; the opencl and output resolvers had each grown a
  sibling-first lookup of their own to dodge exactly that. All 39 lookups go
  through it now.
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
`project.json` were converted in place. The `opencl` node itself still
ships and still runs; retiring it and both kernel backends is the rest of
step 3, and `test_loader_merges_new_template_params` is the migration's
test.

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
where the `opencl` node used to be the only one. The engine is a dependency;
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
`kernel_cpu`'s step budget as a setting rather than a hand-rolled counter,
and `RUN_BUDGET` seconds of wall clock for the whole run, checked in
`on_progress` every few thousand operations. Any failure — syntax, a runtime
error on an element, a budget — fails the WHOLE run, named by node and
element (`wrangle1: point 4: …`), and the input passes through untouched: a
half-wrangled geometry is not a result. Compiled scripts cache by desugared
source in a thread-local, as `OPENCL_CACHE` keys kernels.

CPU only, deliberately: an interpreter is an order of magnitude or more
below native Rust, which is fine for tens of thousands of elements per edit
and wrong for a solver at a million per frame. That is Phase 7's step 4
(WGSL compute through the renderer), not a reason to grow this.

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
currency is a `Page` — a printed sheet: inches, a DPI, and straight-alpha RGBA
pixels — its origin is the top-left corner with y running DOWN, and nothing in
it has a point id, an attribute or a normal. Four nodes compose one: `page`
(the sheet: preset or custom size, orientation, resolution, colour),
`page_grid`, `page_border` and `page_text`.

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
299.9994). Inches rather than millimetres because paper is specified in inches
by the family this came from; the geometry graph's World Unit declaration does
not reach here.

Rect coverage is exact area, not a test of the pixel centre. A printed grid is
mostly hairlines, and a binary fill snaps every rule to whole pixels, so a
ruled sheet comes out with lines alternating between one and two pixels wide
down its length — which reads as a wobble in the paper rather than as
aliasing. Grid rules are centred ON their coordinate so a second grid at twice
the cell size lands exactly on the first's, which is the only reason to draw
two. Text shapes and rasterizes through cosmic-text, the toolkit's own font
stack, with system fonts loaded because a page names its font by family.

**The preview pane** (`PAGE_IDX`, an `ImageView`) takes the viewport's rect
when the displayed level holds a page, and the viewport stands down — the same
rule the viewport already follows about showing its editor's level. Three
things were needed to make a new pane actually appear, and missing any one of
them looks identical to the others:

- A `PAGE_IDX` arm in `paint_widget`. The fall-through branch serves LEGACY
  widgets — it emits a plate and the widget's legacy views — so a modern-paint
  widget whose whole look lives in `Paint::paint` lands there and draws
  nothing. The pane was visible, correctly placed and blank.
- The viewport's key in the `draw_order` sort. The viewport is full-bleed and
  the other panes float OVER it, so a pane taking its rect must take its depth;
  drawn last, it covered the collapsed stubs and their labels ghosted through
  from the later text pass.
- An entry in `test_widget_roster_indices_are_dense`, which is hand-listed and
  fails loudly — the one of the three that tells you itself.

The GPU image is owned by `State::page_image` and freed when replaced;
`ImageView` only borrows the id. **A replacement renderer invalidates that id.** There is no reconnect
callback: the runner calls `renderer_init` once per renderer, so the first call
is this process's own and every later one is a replacement — remembering is the
only way to tell them apart (`State::seen_renderer`, via
`renderer_handed_over`, which is split out of the callback so it can be tested
without a live `VkRenderer`). Images uploaded outside that callback are not
replayed, so a cached id names nothing and its draws are skipped in SILENCE:
the page pane just goes blank. The id is dropped and `page_dirty` asks the next
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
everything-at-once node, is deliberately not ported: it is these four chained,
and that collapse is the whole premise of "fifty operators, ten nodes".

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

The widget paints the lattice as lines (`paint_grid`: gap colour, network
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
Ctrl+P LANDS on Commands, Alt+D toggles the dialog as a whole.

`test_every_menu_command_names_a_label_that_is_dispatched` scans `app.rs` for
`execute_menu_action`'s arms. Scanning source is an odd way to assert it, but
the alternative is calling every command to see whether it is handled, and
"Exit" would end the test run. It is the check the plugin's `hccommands.py` doc
argues for: a label kept in two places drifts, and a renamed one fails silently
— the dispatch falls through its match and the command does nothing.

### The dialog (Alt+D, Ctrl+P, Tab)

`src/dialog.rs` is the app's one modal overlay, and **every filterable list in
the designer is now an opening of it**. It is two roster slots — `DIALOG_IDX`,
an app-owned `Dialog` that paints the plate, the header, the query line and the
row list, and `DIALOG_PARAMS_IDX`, a **second `ParametersBg`** laid out inside
it. The settings controls are that second params pane rather than new widgets,
because a slider in the dialog should be the same slider as a slider in the
params pane; what the dialog adds is the row TABLE, not the rows.

`Mode` says what an opening is for, and it is the reason there is one widget
rather than two:

- `Mode::Tabbed` (**Alt+D**, and **Ctrl+P** onto Commands) — a tab strip over
  two halves. **Commands** is the registry fuzzy-filtered in place;
  **Settings** is the viewport/graph display state `DesignSettings` persists.
- `Mode::AddNode` (**Tab**, in the network pane) — one list of node templates,
  a title where the strip goes, and a pick that instantiates at the grid
  cursor. No tabs: adding a node is a contextual act, not a peer of the app's
  settings. Tab is what opened it, so Tab closes it again.

Both modes share the plate, the keys and `fuzzy_rank`, which is the whole
point — the app used to put two filterable lists in front of the user that
looked and behaved nothing alike.

**Toggle commands are switches in the Commands list, and picking one does
not close it.** `State::command_toggle_state(id)` is the table: it reads the
same field each toggle command flips (the read the View menu's checkmarks
are set from), and a row it answers carries a `toggle` that paints as the
toolkit's own `Toggle` in a right-hand column reserved for every row, so the
chord column keeps a straight edge. Enter or a click on such a row runs the
command, re-reads the switches in place (`refresh_dialog_toggles` — not
`refresh_dialog_rows`, which re-ranks and would throw the selection to the
top) and leaves the dialog up: Show Grid, Show Cube and Square Aspect are set
together while looking at the viewport. Snapping is a switch only inside a
viewer state. `dialog_toggle_rows_cover_every_toggle_command` fails when a
`toggle_*` / `show_*_pane` command is added without an arm in the table,
because the miss is silent — the row just ships plain.

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

**The zoom slider is a row of the Commands list, not a command.** While the
network pane is focused — and only then, since zoom is that pane's — the
list heads with a "Zoom" row (`ZOOM_ROW_ID`) carrying a `slider`: the
toolkit's own `Slider`, painted from one stamp over the row's right end (it
borrows the chord column rather than reserving `SLIDER_W` on every row). The
band **begins `SLIDER_W` in from the row's right end and runs out to the CHORD
column's right edge** — so it ends where every other row's key binding ends
rather than stopping short of them, and the switch column stays clear; that
edge moves with `toggle_col`, which is why the rects are methods rather than
associated functions. The percentage readout therefore sits AHEAD of the band,
the one place left for it, and a press tests the band alone — over the whole
control a click on the readout would jump the value to whichever end of the
range it abuts. It reads the network zoom as a percentage of the configured
grid pitch (`State::zoom_percent`, 100 = Reset Zoom, range the pitch limits).
A press on the band jumps to it and arms the app's widget-drag protocol on
`DIALOG_IDX` (`Dialog::draggable` / `drag_*`, exactly as the Settings
half's sliders arm it on `DIALOG_PARAMS_IDX`), so the value follows the
pointer off the plate; the drained value lands through
`State::set_zoom_percent`, which zooms about the cursor cell and re-reads
the row, since `zoom` clamps. The wheel over the control turns it (2% of
the range a notch, up meaning in — the viewport zoom wheel's sign) where
over the rest of the list it scrolls; `dialog_mouse_wheel` drains the
change like a click. Left/Right nudge it by a Zoom In / Out step while it
is selected; Enter on it runs nothing. The dialog stays up
throughout, as it does for the toggle rows. Ranked like a row labelled
"Zoom", so a query still finds or drops it.

**Alt+D, not Super+D.** Every Super chord is the compositor's before any client
sees one (`input.kdl`'s `cce-window-manager` domain has `super+d` on the app
launcher), and Super held is the DE's window-adjust modifier besides. Alt is the
app's own — the `move_*` family already lives there.

**A Settings row edits the live field** — see "There are no meta nodes"
above, which is where these values used to live and why a direct write did
not stick. `SETTINGS` is the table of which row belongs to which owner, and
`Owner` has three arms for the three kinds there turn out to be: a live
field (`Field`, with a `Ctl` saying what control draws it, since a bare Rust
field carries no type or range the way a param did), a registry command
(`Command` — Square Aspect and Show Camera Pivot are per-CAMERA, with no node
at all behind the Default Camera, and their commands are the only code that
gets both cases right), and an active-camera param with the live field as its
fallback. Writeback is `sync_parameters_to_project`'s shape, polled rather
than pushed for the same reason: a `ParametersBg` reports its values, it does
not emit events. `dialog_settings_rows_name_owners_that_exist` is the
backstop, because the failure is silent — a `Field` key no dispatch arm names
reads a default and writes nowhere, so the row draws, takes an edit and does
nothing, which is why that test round-trips every one of them.

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
truncates itself; the settings body is painted twice for this (once for its
controls, once to re-emit its text retagged), since a `PaintCtx` can be handed
text back but not geometry.

**`Dialog::occluding` exists because that one claim serves two mechanisms that
want opposite answers.** `UiContext::is_coordinate_covered` reads the same
`popover_rect` — off every REGISTERED widget, not just the ones in
`active_popovers` — to decide a press landed under something else. With the claim
standing, every control in the Settings half is covered by the plate it is drawn
on and nothing can be clicked; the toggles looked laid out, painted, and
completely inert. `State::dispatch_uncovered` lowers the flag for the length of a
dispatch into the dialog and puts it back, invalidating the coverage memo on both
edges (the engine queries it on every left press, so lowering the flag alone
leaves a stale cached answer).

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
`Some` in a long time. `cce_ui::process::CloudPopup` itself still exists; the
designer was its only consumer, so it is now unused public API in a shared
crate, which is a coordination job of its own.

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
went native) refreshes its children's params and any kernel child's `Code`
outright — **the template owns the surface and implementation, the
instance owns its values.** A kernel hand-edited inside a template
instance reverts on load; custom kernels belong in bare OpenCL nodes,
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
