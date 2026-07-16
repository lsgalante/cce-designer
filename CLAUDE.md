# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-designer` is a node-based procedural 3D design app (Houdini-style) for the cce
desktop environment: a node graph is evaluated into geometry — OpenCL kernels do the
generation — and displayed in a 3D viewport with both a raster pass and a path-traced
(RT) preview mode.

This crate is one member of the multi-repo `cce` Cargo workspace; workspace-wide rules
(multi-repo layout, no `[workspace.dependencies]`, shared `../target/`) live in
`../CLAUDE.md`. This directory is its own git repository.

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
kernels and need a working OpenCL runtime; they are not pure-CPU tests.

### CLI modes

- `cce-designer --thumbnail <project-dir-or-state.json> <out.png> [--size N] [--samples N]`
  — headless path-traced thumbnail (no Wayland, no window; `src/thumbnail.rs`).
  `cce-files` shells out to this for its preview cache.
- `cce-designer --detached-network` — a separate network-pane-only window. It syncs
  with the main window by autosaving/polling `default_project.json` mtime (see the
  main loop in `src/main.rs`) — there is no socket between the two.

### HTTP automation API + MCP server

The main window runs an embedded HTTP server on `127.0.0.1:3000` (`src/api.rs`):
`GET /state` returns the app state as JSON; `POST /action` takes an `HttpAction`
JSON body (`add_node`, `set_param`, `menu_action`, `save`, `load`, …— see the enum
in `src/app.rs`). This is the main way to drive/inspect the running app when
debugging: `curl -X POST localhost:3000/action -d '{"action":"add_node","template_name":"Sphere","x":5,"y":3}'`.

The same surface is exposed as an MCP server on `127.0.0.1:3001`
(`CCE_DESIGNER_MCP_PORT` overrides; attach with
`claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`):
one tool per `HttpAction` variant (tool name = the variant's serde tag,
dispatched in `apply_mcp_call` in `src/window.rs`) plus `get_state`. The tool
list lives in `mcp_tools()` in `src/api.rs`; the protocol layer is
`cce_ui::mcp` (tools-only Streamable HTTP). Keep the enum, the tool list, and
the schemas in sync — `test_mcp_tools_map_to_http_actions` enforces the
mapping.

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
engine's shaping/glyph pass (the app has no `FontSystem` or buffer cache of its own;
`glyphon` remains a dependency only for the standalone `vk-smoke` bin).

- `src/app.rs` (~5k lines) — the heart: `State` (the entire app model), `HttpAction` /
  `CustomEvent`, node-template loading, pane layout. Top-level widgets live in fixed
  slots addressed by `*_IDX` constants (`VIEWPORT_IDX`, `PARAM_IDX`, `NETWORK_PANEL_IDX`,
  … up to `WIDGET_COUNT`) rather than a dynamic tree. `tick_frame` (simulation:
  config polling, inertia, widget ticks) and `stage_frame` (renderer staging) are the
  two halves of the old render loop. GPU mesh updates are staged CPU-side
  (`pending_*` fields, `spheres_dirty`) and flushed in `stage_frame` because only
  the engine hooks see the renderer.
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
- `src/geometry.rs` — node-graph evaluation. Each OpenCL node's kernel code is
  preprocessed: `chf("name", default)` / `chi` / `chv` calls are parsed into dynamic
  UI parameters (`parse_dynamic_params`) and rewritten to `param_values[i]` reads
  (`preprocess_opencl_code`). `network_sphere_vertices_with_errors` walks the graph
  from output nodes; OpenCL failures are collected, not fatal.
- `src/viewport_3d.rs` — app-owned `Viewport3D` widget (camera orbit/zoom, inertial
  scroll, `rt_mode` flag switching the pane to the `cce_ui::vk` compute path tracer).
- `src/project.rs` — save/load. A project is a **directory containing `state.json`**
  (`Project { name, root: FsNode, view_state }`); `default_project.json` in the crate
  root is special-cased as a single file and doubles as the detached-window sync channel.
- `src/shortcut.rs` — `Shortcut::parse("Ctrl+Shift+g")` → `Action` mapping.

The `zcce_inspector_v1` integration (window-position tracking + widget-state
streaming to cce-test-interface) was dropped in the engine migration; the HTTP API
is the introspection surface.

### App-written settings: `~/.config/cce/cce-designer/state.kdl`

`DesignSettings` (viewport/graph display state the app rewrites itself:
colors, grid sizes, show flags) persists to `state.kdl` — deliberately NOT
`config.kdl`, which is the user-authored toolkit-config override slot that
cce-ui auto-merges (see `../CLAUDE.md`). Legacy `design.kdl` / `design.json`
files migrate on load. Scroll behavior (`scroll_speed`, `inertial_scroll`,
`scroll_friction`) is intentionally absent: it is config-owned
(`input.inertial` in config.kdl) and must not be shadowed by app state.

### Runtime paths point into the source tree

Node templates (`nodes/*.json`) and `default_project.json` are located via
`env!("CARGO_MANIFEST_DIR")` — the installed binary still reads from the source
checkout. Templates are resolved recursively: a template's children reference other
templates by `type`, merged with param overrides (`load_fs_tree` in `src/app.rs`).
Missing referenced templates panic at load.

## Repo hygiene

`scratch/` holds ad-hoc debug scripts/logs and `screenshot*.png` at the root are
debugging artifacts — not source, don't extend them. All tests live in
`src/main.rs`'s `#[cfg(test)]` module; add new ones there. Commit messages follow
`feat:` / `fix:` / `refactor:` style (see `git log`).
