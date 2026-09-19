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
- `src/curve_tool.rs` — the curve viewer state (Houdini-style viewport point
  editing for the native `curve` node; "Edit Points" in the node context menu).
  The pattern for any future viewer state: project world positions to handles
  through `State::last_scene_mvp` + `last_scene_view_rect` (both LOGICAL px,
  the rect is divided by scale where it is cached — same path as the meta
  Point Numbers overlay), hit-test against `cursor_x/y`, drag by unprojecting
  the cursor at the grabbed point's captured NDC depth, and write edits back
  through the SetParam resync sequence (`sync_nodes` + `rebuild_scene_geometry`
  + `sync_parameters_pane`). Input hooks live in `handle_event`: presses
  intercept in the MouseInput arm ahead of the viewport context menu (gated on
  `cursor_in_viewport() && !in_network_pane`, so the network plate keeps its
  clicks where they overlap), motion at the top of CursorMoved, Escape ahead of
  connection-cancel. The tool binds the node by ID and deactivates lazily when
  the id no longer resolves to a curve. Undo/redo is a
  `cce_ui::history::History` of point-list snapshots on the tool, one per
  gesture (a drag opens a gesture its first motion commits, so a no-move
  click leaves nothing). The chords are the toolkit's (`undo`/`redo` in
  input.kdl, routed by the runner to `Application::undo`/`redo` in
  `application.rs` after the focused text box declines); Edit ▸ Undo/Redo
  reach the same code through `Action::Undo`/`Redo`. There is no
  project-wide history yet; `execute_action` is where one would be consulted
  after the tool declines.
- `src/project.rs` — save/load. A project is a **directory containing `state.json`**
  (`Project { name, root: FsNode, view_state }`); `default_project.json` in the crate
  root is special-cased as a single file and doubles as the detached-window sync channel.
- `src/shortcut.rs` — `Shortcut::parse("Ctrl+Shift+g")` → `Action` mapping.

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
