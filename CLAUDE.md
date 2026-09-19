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
make install                           # release build → ~/.local/bin/cce-designer
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

- `src/app.rs` (~5.4k lines) — the heart: `State` (the entire app model), `McpAction` /
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
- `src/plate_corner.rs` — the plate corner control: a circular menu trigger on the
  top-right of each pane that draws its own plate (`PLATE_SLOTS` — network, params,
  spreadsheet, playbar; NOT the viewport, whose plate is the window-spanning lip).
  Geometry is derived from the slot's live rect, so it holds across all three
  `rebuild_positions` branches; the circular network pane is special-cased onto its
  arc. The menu is a third `cce_ui::widget::context_menu` consumer alongside the node
  and viewport right-click menus, with the same `*_menu_actions` + `handle_*_menu_click`
  contract. Collapse shrinks a plate to its title stub via `apply_collapsed_panes`, a
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
  on top of the solved result; dived INTO a simnet, the walk draws the solved
  state instead (toggled by the output child's geometry flag), and at any
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
  the rect divided by scale where it is cached — the same path as the meta
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

### The root meta node (nee Session)

Session-wide settings live under one permanent root node: `meta` (node type
`meta` — retyped/renamed from the old `Session`/`session` on load, children
intact) contains the Main/View/Guides/Render utility subnets that used to
sit flat in `/`. It is the root network's counterpart of every node's
per-node `meta` child, but still a subnet. `ensure_menubar_subnets` creates
it and MIGRATES older saves into it (root-level settings nodes moved, not
recreated — params survive; a `session`-typed container retypes in place).
It cannot be deleted: `delete_node` refuses the `meta` (and legacy
`session`) type — the one gate every deletion route funnels through — the
context menu omits Delete, and the graph draws it without a geometry
toggle. `State::session_node()` / `in_settings_dir()` are the accessors —
the latter walks the whole `current_path`, since a first-segment check
stopped working the day the settings nodes gained a parent. Guides holds
"Point Marker Size" (thousandths of a world unit), driving the per-node
meta Point Markers overlay via `State::meta_marker_size`, and **"World Unit"**
(a `choice`: mm / cm / m / in, `State::world_unit`) — what one world unit IS.
Geometry never converts; the declaration feeds two things through the display
metric (`cce_ui::units`): the viewport's bottom-left **scale readout**
(`append_scale_readout`: `1:2.3`, `1 mm = 0.43 mm on screen`, marked when the
metric is only assumed) and the viewport context menu's **View 1:1**
(`view_one_to_one`), which moves the active camera along its eye ray so the
pivot plane shows one world unit at its true length — the default camera by
zoom, a camera node by rewriting its Position, as Frame All does. The
projection is a perspective (vertical FOV 0.9 rad), so 1:1 holds on the
pivot plane only; `view_scale_ratio` is the readout's number.

### The meta node (per-node preferences)

Every geometry-producing node carries a **`meta` child** (node type `meta`) —
per-node preferences, edited by entering the node and selecting it. Current
prefs: "Point Markers" and "Point Numbers" (viewport overlays on that node's
output; numbers project through the cached raster mvp into the 2D text pass,
`append_meta_point_numbers`). `ensure_meta_on` / `ensure_meta_children`
(src/app.rs) create it at instantiation and at every project load — the same
migration pattern as the Session node — and also restore missing pref params,
so adding a pref is one entry in `ensure_meta_on`'s list plus its consumer.
`meta_pref(node, name)` is the read. Meta is undeletable (the `delete_node`
gate alongside `session`), shows no geometry toggle, and is invisible to
evaluation. Overlay data rebuilds with the scene (`collect_meta_overlays` in
src/render.rs, walked with the scene's visibility chain).

### App-written settings: `~/.config/cce/cce-designer/state.kdl`

`default_project` in state.kdl points at the project the main window opens on
startup (the Main node's File > "Set As Default" button; absent = the bundled
`default_project.json`). It is a POINTER, never a rewrite of
default_project.json — that file is versioned and is the detached-window sync
channel. A default whose path no longer exists is dropped from the settings on
launch. Detached windows ignore it: they must keep seeding from the sync
channel.

`DesignSettings` (viewport/graph display state the app rewrites itself:
colors, grid sizes, show flags) persists to `state.kdl` — deliberately NOT
`config.kdl`, which is the user-authored toolkit-config override slot that
cce-ui auto-merges (see `../cce-compositor/WORKSPACE.md`). Legacy `design.kdl` / `design.json`
files migrate on load. Scroll behavior (`scroll_speed`, `inertial_scroll`,
`scroll_friction`) is intentionally absent: it is config-owned
(`input.inertial` in config.kdl) and must not be shadowed by app state.

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
`ImageView` only borrows the id. `gem_graph`, the source family's
everything-at-once node, is deliberately not ported: it is these four chained,
and that collapse is the whole premise of "fifty operators, ten nodes".

### Keyboard graph navigation

The network pane's keyboard scheme is the plugin's, ported: **hjkl rather than
arrows** — the arrows are the playbar transport in every pane and context — bare
to move the grid cursor, `alt` to move the node under it, `ctrl` to pan the
view, plus `f` to frame the cursor and `shift+f` to frame everything. All
fourteen are registry commands in `Context::Network`, so they are rebindable
through `input.kdl` and listed in the palette.

**The grid cursor IS the selection.** `sync_cursor_and_selection` selects
whatever node sits in the cursor's cell, so navigating selects, and stepping off
a node deselects. Every family is gated on the network pane having focus — one
gate, in the four `network_*` methods. The bare family used to be the one that
was NOT gated: plain h/j/k/l moved the cursor from any pane, so it drifted
invisibly while you were looking at the viewport (the selection did not follow,
because `sync_cursor_and_selection` has its own pane check) and was somewhere
unexpected when you came back.

`alt` moves the node AND the cursor, so a run of `alt+h` drags a node across the
sheet rather than leaving it behind on the first press. `ctrl` pans by one CELL
rather than a fixed pixel count, so a pan step means the same thing at every
zoom. Frame Cursor CENTRES the cursor cell; its first version called
`keep_cursor_in_view`, which pans only when the cursor has gone off an edge, so
the command did nothing at all in the common case of a cursor that is visible
but off in a corner — which is exactly when it gets pressed.

`shift+hjkl` — the plugin's extend-the-selection family — is deliberately
absent. The Graph widget carries a single `selected_node`, so four rows that
quietly did what bare hjkl already does would be worse than the gap.

Two chords moved to make room, both caught by `command::conflicts` rather than
by hand: `edit_handles` from `Ctrl+H` to `Ctrl+Shift+H` (the ctrl+hjkl family
owns those now), and `f` now frames the CURSOR where it used to frame
everything, with framing everything on `shift+f` — the plugin's split.

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

**The palette** is the node palette's mechanism, not a new widget: the same
`cce-cloud --dmenu` popup at the cursor, with the same keys. Two pickers in one
app that look and behave differently is worse than either. Ranking is
`fuzzy_rank`, which reproduces the plugin's fuzzyfinder exactly — shortest
contiguous span, then earliest start, then alphabetical — so muscle memory
survives the move; the focused pane's commands are then partitioned to the
front, stably, without dropping anything (a palette that hides what you are
looking for is worse than one that lists it second). Rows are the label padded
to a column and then its chord, so the palette teaches the keyboard rather than
replacing it; padded rather than tab-separated because the popup renders a tab
as one literal stop and the chords came out ragged. A row is matched back to its
command by the LONGEST label it starts with, since "Save" starts "Save As"'s
row.

`test_every_menu_command_names_a_label_that_is_dispatched` scans `app.rs` for
`execute_menu_action`'s arms. Scanning source is an odd way to assert it, but
the alternative is calling every command to see whether it is handled, and
"Exit" would end the test run. It is the check the plugin's `hccommands.py` doc
argues for: a label kept in two places drifts, and a renamed one fails silently
— the dispatch falls through its match and the command does nothing.

### Runtime paths point into the source tree

Node templates (`nodes/*.json`) and `default_project.json` are located via
`env!("CARGO_MANIFEST_DIR")` — the installed binary still reads from the source
checkout. Templates are resolved recursively: a template's children reference other
templates by `type`, merged with param overrides (`load_fs_tree` in `src/app.rs`).
Missing referenced templates panic at load.

Saved instances are self-contained copies, but the loader merges template
evolution into them (`merge_template_defs` in `src/app.rs`, run on every
project deserialization including thumbnails): missing params are appended,
existing ones keep their value but take the template's UI metadata, and subnet
templates (Sphere/Plane/Extrude) refresh their children's `Code` outright —
**the template owns the surface and implementation, the instance owns its
values.** A kernel hand-edited inside a template instance reverts on load;
custom kernels belong in bare OpenCL nodes, which the merge never touches.
Native nodes match their template by type, subnet instances by name
("Sphere 3" → "Sphere") plus a full child name/type match; the merge never
injects or deletes children and never rewrites files on disk.

## Repo hygiene

`scratch/` holds ad-hoc debug scripts/logs and `screenshot*.png` at the root are
debugging artifacts — not source, don't extend them. All tests live in
`src/main.rs`'s `#[cfg(test)]` module; add new ones there. Commit messages follow
`feat:` / `fix:` / `refactor:` style (see `git log`).
