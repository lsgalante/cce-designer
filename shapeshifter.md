# Shapeshifter in cce-designer

Proposal v0 | September 2026

Bringing the Developer, Immutable Methods and GEM toolsets out of `hou-control`
and into this app — which is mostly not a porting job. It is one data-structure
decision, then about ten nodes that do the work of fifty.

The source material is `~/projects/hou-control`: its `developer.md` (the
Shapeshifter design document), `otls-audit.md` (every HDA, its parameters, and
which of them nothing reads) and `shortcomings.md`.

## What each side already has

The plugin is three tool families on top of an interaction layer. The app is a
working procedural pipeline with no vocabulary yet. They meet in fewer places
than the node counts suggest.

**hou-control, today**

- ~50 `developer_*` HDAs — attribute solvers, surface development, the Solver
  and its Vis tabs.
- ~90 `im_*` HDAs — the general modeling vocabulary across Create, Topology,
  Move, Filter, Analysis, Layout.
- ~30 `gem_*` HDAs — mold, sprue, build area, supports, plus a COP family for
  printed output.
- 8.8k lines of `hc` — keycam navigator, HC Panel, hotkey JSON, network editor,
  settings schema.

**cce-designer, today**

- Graph evaluation with feedback — `simnet` already iterates a chain, caches per
  frame, and restarts on edit.
- Two kernel backends — OpenCL, plus the CPU interpreter in `src/kernel_cpu.rs`
  that is the semantic reference.
- The panes — network, parameters, spreadsheet, playbar, viewport; detachable,
  pinnable, saved in the project.
- Declared world units — mm/cm/m/in, a scale readout, View 1:1.

**Not there yet**

- Points, primitives and detail — there is only a vertex list.
- Any notion of a neighbour, an edge, or a stable point id.
- Attributes on the GPU — kernels see positions and colors, nothing else.
- Remeshing, collision, volumes, mesh export.
- A command palette, a hotkey file, a viewer-state framework beyond the one
  curve tool.

## The one thing that blocks everything

Every operator in the Developer set is a statement about a point and its
neighbours. Diffuse averages toward neighbours. Migrate moves value along edges
and the sender loses what the target gains. Concentrate sharpens against
neighbours. Lead turns each vector toward the neighbouring vector that disagrees
most. Analysis writes the spread of edge lengths.

`src/geometry.rs`:

```rust
pub struct Geometry {
    pub vertices: Vec<GVertex>,   // a triangle soup
}

pub struct GVertex {
    pub pos: [f32; 3],
    pub col: [f32; 3],
    pub attributes: HashMap<String, GAttribute>,   // per triangle corner
}
```

Three corners of a triangle are three unrelated entries. A point shared by six
faces appears six times with six independent copies of every attribute. There is
no way to ask what a point's neighbours are, no edge to measure, and no identity
that survives a frame. Groups are faked as `group:name` keys in the attribute
map; attributes are `Float` through `Float4` only, so there are no integers for
counters or ids.

Nothing in Phases 1 through 3 can be written against this, and a workaround —
welding on demand inside each operator — would put an O(n log n) rebuild inside
every node of a chain that runs once per simulation step. So the proposal starts
there, and the first phase is the expensive one.

## Phases

Ordered by dependency, not by appeal. Phase 5 is deliberately detached — it
touches none of the geometry work and can be picked up in any gap.

### Phase 0 — A real geometry model

*Largest. Blocks Phases 1, 2, 3, 4 and 6 — all of them.*

> **Mostly landed.** `src/detail.rs` holds the container — points, vertices,
> primitives, detail; columnar attributes with integers and real groups; stable
> `PointId`s; lazily built CSR topology. The generators build it, every operator
> works on it, and the pipeline's currency IS a `Detail`: the spreadsheet lists
> points, the overlays read the point and edge lists, and `weld_points` is gone.
>
> What remains: kernel GENERATORS still emit a corner list that welds on the
> way back, so `detail_to_soup` / `soup_to_detail` and `geometry::Geometry`
> survive to serve them. Deformers no longer go near a soup (see Phase 1).

Replace the vertex list with **points, vertices, primitives and detail**, each
carrying its own columnar attribute arrays — one `Vec<f32>` per named attribute
rather than a `HashMap` per corner. Columnar is not a nicety here: it is the
shape a GPU buffer already wants, so Phase 1 becomes a pointer instead of a
marshalling pass.

Three things ride along. A **stable `id`** on points, allocated once and
preserved through every operator that does not create points — the thing every
solver needs and the thing a triangle soup can never have. A **lazily built
topology cache** (point→point, point→prim, edge list) invalidated on topology
change, so a chain of ten attribute nodes builds it once. And **real groups and
integer attributes**, retiring the `group:` key convention.

Triangulation becomes a render concern: `to_vertex3d_vec` is already the
boundary, and the raster and RT paths keep consuming triangles.

Touches: `geometry.rs`, `kernel_cpu.rs`, `render.rs`, `curve_tool.rs`, the
Spreadsheet, the meta overlays, `project.rs`.

### Phase 1 — Attribute algebra, and attributes on the GPU

*Large. Needs Phase 0. Blocks Phases 2 and 3.*

> **Started.** The ABI is widened for DEFORMERS: the work item is a point, not
> a triangle corner, and a kernel reaches named float attributes through
> buffers of its own — `attrf("mass", i)` reads, `setattrf("mass", i, v)`
> writes, naming one creates it. Landed in both backends together and held to
> each other by a cross-backend test. A deformer no longer flattens or welds,
> so topology, groups and point identities pass through untouched.
>
> The `neighbour` node is in, as a native Rust evaluator: Diffuse,
> Concentrate, Migrate and Bleed over a Neighbourhood of connectivity rings,
> radius or global, componentwise on any attribute type. Per decision 1 the
> neighbourhood walk stays out of the kernel language.
>
> The attribute vocabulary is in too. `attribute` grew Remap, Clip, Normalize,
> Composite and Promote alongside Create/Modify/Delete; `analysis` writes min,
> max, sum, average, spread and count to DETAIL attributes — five ordinary
> attributes where Houdini writes an `<attr>_info` dictionary, which is
> decision 3 paying off; `time` runs 0 to 1 across a frame range. The
> spreadsheet shows detail attributes as `d:` columns.
>
> Align and Lead landed too, so `neighbour` is the full 7 → 1. Charge is in
> the node as well, but it is NOT a port: `developer_charge` is an empty shell
> in hou-control (two parameters, neither read), so what is implemented is the
> reading its parameter names suggest — accumulate, and discharge to the
> neighbours on crossing a threshold. Confirm or redirect it.
>
> Outstanding: the generator ABI, still a corner list out.

Widen the kernel ABI from `(in_pos, in_col, out_pos, out_col, params)` to
**named attribute buffers bound by the node**, plus the topology arrays as
read-only buffers so a kernel can walk neighbours. This is the change that has
to land in both backends at once — the OpenCL launcher and the CPU interpreter
that `cpu_matches_opencl_on_every_shipped_kernel` holds it to.

On top of it, the attribute vocabulary: **Initialize, Remap, Clip, Composite,
Promote, Analysis** and the `time` family — and one `neighbour` node whose Mode
covers Diffuse, Concentrate, Migrate, Bleed, Align and Lead, over a
Neighbourhood of connectivity, radius or global. That single node is most of the
`developer_*` set.

Touches: `geometry.rs`, `kernel_cpu.rs`, `nodes/*.json`.

### Phase 2 — The solver contract

*Medium. Needs Phases 0 and 1.*

> **Started.** Live and derivative data is a declared property of the
> ATTRIBUTE (`AttribKind`), set where it is created rather than listed on the
> solver. The step boundary zeroes every derivative attribute going in, and
> coming out restores any live attribute the chain DROPPED, matching points by
> identity — so a node that rebuilds geometry mid-chain no longer takes the
> simulation's memory with it. Restoration bridges a rebuild, not a delete: an
> unchanged point set means a missing attribute was removed on purpose.
> Analysis and Time write derivative; the spreadsheet marks them with `~`.
>
> `visualize` is in: Ramp maps a scalar through one of four built-in ramps
> into `Cd`, Vector stages a vector attribute as viewport markers. Several
> attributes at once come from CHAINING Visualize nodes, each blending into
> the `Cd` it was handed, which is how the plugin's Solver Vis tabs work.
> Range Auto re-measures every run, because a simulation's interesting range
> moves every frame.
>
> Substeps run the chain N times per frame, so step size stops being tied to
> the frame rate. The solver writes `dt` (= 1/substeps) as a derivative detail
> attribute; a chain that scales its rate by it — Promote onto points, then
> Composite — covers the same ground however finely the frame is cut, which is
> what makes substeps a stability control rather than a speed control.
>
> A simnet can declare its own Start Frame (empty follows the timeline), and
> opt into a disk cache — the solved state parked under `$XDG_CACHE_HOME`, in a
> compact binary form `Detail` reads and writes itself, keyed by the same hash
> that invalidates the in-memory cache.
>
> Phase 2 is done.

`simnet` is already the Developer Solver — feedback stack, per-node cache keyed
on the subtree, restart on edit, one step per played frame. What it lacks is a
contract for what survives a step.

Make **live and derivative data a first-class distinction** rather than a
convention: an attribute is declared live (carried across the step boundary by
id) or derivative (zeroed at the start of every step). Derivative attributes
then cost nothing to get wrong, and a remesh in the middle of a chain has a
defined answer for every attribute it did not create — which is exactly what
`Surface Remesh` feeding `Develop` needs.

Then: substeps, an explicit seed frame, a disk cache so a long solve survives a
restart, and **Visualize** — attribute-to-color through ramps, several
composited at once, and vectors drawn as markers. The per-node `meta` overlays
already do the projection work this needs.

Touches: `geometry.rs`, `app.rs`, `render.rs`, `playbar.rs`.

### Phase 3 — Surface development

*Large. Needs Phases 0, 1 and 2.*

> **Started.** `develop` displaces along the point normal by an attribute, and
> `remesh` is in as its own module (`src/remesh.rs`): split, collapse, flip and
> tangential relax, the Botsch–Kobbelt passes. It carries the simulation's data
> across — a split interpolates, a collapse keeps the survivor's identity and
> values, and a group only grows where both parents were members.
>
> Fixed on the way: the native generators wound BACKWARDS. Every normal on a
> native sphere pointed into it, and every face of a native box wound against
> the `Norm` it shipped. The template meshes were always right, which is why
> the winding test and the overlay test both passed; a path tracer shades both
> sides, so nothing looked wrong. Develop is the first operator whose answer
> depends on it, and it grew the surface inward.
>
> Collision is in. `detangle` is a point repulsion over Iterations passes with
> Thickness in edge lengths and a Rings exclusion — the audit's own
> description of the Shapeshifter algorithm. `suture` resolves against a second
> input, counts SUSTAINED contact, and fuses points past Fusion Threshold
> within Distance Threshold. Both, and the remesher's projection pass, run on
> `src/spatial.rs` — a uniform grid built once for all three.
>
> **`adapt` and `open` are not ported and should not be guessed at.**
> `developer_surface_adapt` has eight read parameters and no description
> anywhere; `developer_surface_open` has none at all. Unlike Charge, whose
> parameter names carried a reading, these carry nothing. Say what they do and
> they are a short job each.
>
> `subdivide` is in: four triangles where there was one, attributes
> interpolated onto the midpoints, and the shape left exactly where it was —
> it refines, it does not smooth, which is what separates it from Remesh.
>
> Phase 3 is done but for the two operators nobody can describe.

`Develop` is easy — displace along the normal by a development attribute.
**Remesh is the hard one**, and it is load-bearing: without topology that keeps
primitives proportional to surface area, every growth sim degenerates within a
few dozen frames. Incremental remeshing (split long edges, collapse short ones,
flip toward valence 6, tangential relaxation, project back to the input surface)
is well-trodden ground and should be its own module with its own tests, not a
node body.

Around it: `Subdivide`, `Adapt`, `Open`, and collision — `Detangle` as point
repulsion between non-neighbours to a thickness, and `Suture` resolving against
the previous frame and fusing what keeps colliding. Both want the spatial index
Phase 0's topology cache should already own.

Touches: a new `remesh.rs`, `geometry.rs`, `nodes/*.json`.

### Phase 4 — The modeling set

*Wide, shallow. Needs Phase 0. Runs parallel with Phases 2 and 3.*

> **Started.** The measure-and-filter five: `normal` publishes the surface
> normal as an attribute anything can read, `bounds` and `distance` measure
> (the latter writing a direction too, out of the same lookup, which is what
> Migrate flows along), `connectivity` numbers pieces largest-first, and `cull`
> deletes — the first node in the app that removes geometry.
>
> The IM family carries no prose anywhere, but unlike `adapt` and `open` these
> names are unambiguous: a node called Normal computes normals. Where the audit
> recovered parameter names from the HDAs they are honoured (`piece_attr`,
> `dir_attr`).
>
> Copy and Soft Transform are in too, and `group` grew Attribute and Expand —
> selection by what a point IS rather than where it is, which is what makes
> the measuring nodes composable, plus grow/shrink across the surface.
>
> `points` and `scatter` can now emit BARE POINTS. Both drew marker spheres at
> every location, which is right for looking at and wrong for working with:
> Copy placed one instance per marker vertex rather than one per location,
> because the markers were the only points there were.
>
> `transfer` carries attributes from one geometry onto another by nearest
> point — how a field outlives the geometry it was defined on, which a chain
> that REBUILDS needs and a remesh cannot provide. `valence` publishes the
> number the remesher steers toward. `deform` collapses `im_twist`, `im_bend`
> and `im_curl` into one node: they are the same shape, a transform whose
> strength varies along an axis.
>
> The Create set grew a native `grid` (welded points, no kernel, so it can
> feed a remesh or a diffusion directly) and a `polygon` covering `im_square`,
> `im_triangle`, `im_star` and the circle nobody got round to — one shape with
> one parameter varying.
>
> Remaining: Select (which is `group` with more criteria, not a new node), and
> the long tail — which the audit prunes hard (version forks, the
> dead-on-arrival nodes, the Houdini-SOP wrappers).

The `im_*` family, which is the part that looks biggest and is actually the
easiest — ninety nodes, most of them a screenful once points and prims exist.
Sequence it by what the Developer chain consumes rather than by category:
**Group, Select, Cull, Transform, Soft Transform, Relax, Copy, Scatter, Bounds,
Distance, Neighbours, Connectivity, Normal** first, the Create primitives next,
then Analysis and Visualize.

`otls-audit.md` is the parameter spec — it lists every node's parm count and
flags the 74 unread parameters on `gem_build_area` and the dead ones elsewhere.
Port the surface you meant, not the one that accumulated.

Touches: `geometry.rs`, `nodes/*.json`.

### Phase 5 — The interaction layer

*Medium. Needs nothing — start any time.*

> **Started.** Parameters can declare `show_when`, a condition over their
> siblings' values, and the pane shows only the rows that apply: `attribute`
> drops from seventeen rows to seven, `group` from fifteen to eight,
> `neighbour` from thirteen to seven. This was the cost of the 50 → 10
> collapse coming due — a pane of twelve irrelevant rows is worse than the
> twelve nodes it replaced — and it is the first Phase 5 item because it was
> the binding constraint on using what Phases 1 to 4 built.
>
> **The command registry and the palette landed.** `src/command.rs` is one list
> of everything the app can do — id, label, context, how to run it, default
> chord — and `ShortcutManager` now binds command IDS rather than `Action`s,
> which is what lets a chord reach a menu-dispatched command like Open at all.
> `State::run_command(id)` is the single entry point and is exposed over MCP,
> so every command is scriptable.
>
> The palette is the node palette's `cce-cloud --dmenu` popup rather than a new
> widget: two pickers in one app that behave differently is worse than either.
> Ranking reproduces the plugin's fuzzyfinder exactly — shortest span, earliest
> start, alphabetical — so muscle memory survives; the focused pane's commands
> partition to the front without anything being hidden. Rows carry their chord
> in a column, so the palette teaches the keyboard instead of replacing it.
>
> Two findings the registry paid for immediately. The toolkit's runner already
> claims ctrl+z, ctrl+shift+z, ctrl+tab and ctrl+shift+tab before the app sees
> them, so undo and redo ship with no chord HERE on purpose — binding ctrl+z
> would have taken undo away from focused text boxes while looking like a fix.
> And `Shortcut`'s derived `PartialEq` compared character keys case-sensitively
> while `matches()` compared them case-insensitively: `Ctrl+S` and `Ctrl+s` were
> one keypress at the keyboard and two values in memory, so the new conflict
> detector silently failed to report the very collision it exists to catch.
>
> The hotkey file this phase asked for turns out to be already built and better
> than proposed: `input.kdl` is workspace-wide with per-app domains, so a chord
> is `cce-designer.<id>` there and the registry supplies the default. What was
> missing was not a file but a set of NAMES to put in it, and conflict
> reporting; both are in.
>
> **The viewer-state framework landed.** `src/viewer_state.rs` owns everything
> the curve tool had that was not about curves: projection, hit-testing, the
> drag model, per-gesture undo, binding by node id, write-back — plus the two
> things this phase asked for that it did not have, snapping and a HUD.
>
> What differs per tool is the `HandleSource` trait, and two implementations
> ship because an abstraction with one implementation has not been shown to be
> one. The curve is an open-ended list of world positions stored as world
> positions. The soft transform is a FIXED pair whose second handle is
> `Centre + Translation` — a derived position, converted both ways by the
> source, so the framework's drag maths never learns that one of its two world
> points is not a place. That conversion is the whole reason the trait exists.
>
> The one design question it forced: `write` is handed a full set of handles
> with no word about which moved, because it has to mean the same thing when
> the set came from an undo snapshot as when it came from a drag. So the soft
> transform's handles read as a vector with a base and a tip, and dragging
> either end changes the offset between them. The alternative — keep the
> translation fixed when the centre moves — would be right for the drag and
> would quietly discard half of every restored snapshot.
>
> `source_for` is the one map from node type to tool, so the context menu entry,
> the `edit_handles` command and anything later cannot disagree about what is
> editable: a new source appears in the menu without the menu being touched.
> The entry is now "Edit Handles", not "Edit Points" — only a curve's are
> points. Snapping is a command (`toggle_snap`), which is the previous round's
> registry paying for itself.
>
> Still outstanding for this phase: keyboard graph navigation and auto-layout
> in the network pane. The keycam navigator the proposal names as a viewer
> state is not written yet, but the framework it would sit on is.

Independent of all the geometry work, and the place where the app gets to be
better rather than equal. A **command palette** on the HC Panel's model — fuzzy
search over every action, contextual to the focused pane — which in Houdini
exists partly because there is no API to open the native tab menu. Here it is
just a widget.

A **hotkey file** (kdl, alongside `state.kdl`) with conflict resolution,
extending `shortcut.rs`. **Keyboard graph navigation** and auto-layout in the
network pane. And a **viewer-state framework** generalized out of
`curve_tool.rs`, which CLAUDE.md already names as the pattern: handles,
snapping, a HUD, per-gesture undo. The keycam navigator is a viewer state on
that framework.

One thing gets deleted rather than ported. `hcviewregions.py` publishes pane
rectangles to `ccectl` so the compositor can synthesize a view drag from a
two-finger swipe, because a trackpad gesture never survives Xwayland. A native
Wayland client receives the gesture directly.

Touches: `shortcut.rs`, `app.rs`, `slots.rs`, `cce-ui`.

### Phase 6 — GEM and manufacturing

*Largest. Needs Phases 0 and 3.*

> **Mesh export landed early**, out of order, because everything Phases 0 to 4
> build could until now only be looked at inside the app. `src/export.rs`
> writes STL and OBJ, there is an `export` node and a `--export` CLI mode, and
> a solved growth simulation can be written to a printable file.
>
> **The volume representation landed.** `src/volume.rs` is a dense signed
> distance field; the `volume` node offsets and shells, the `boolean` node
> unions, intersects and subtracts. Extraction is surface nets.
>
> Signing the field cost three wrong answers before a right one. Ray parity
> double-counts at shared edges and inverted 79 of 15625 samples. A flood fill
> from the grid boundary fixes that, but a 0.75-voxel band let it walk through
> a thin wall and a slab came back hollow — the band has to be a full voxel,
> because two samples one voxel apart cannot both be further than a voxel from
> a surface between them. And the band's own test, which asks the nearest face
> which side a sample is on, trusts the winding; a mesh wound inside out came
> back with its band signs alternating against the flood's, so the winding is
> now measured by the divergence theorem and the test flips to match.
>
> The limit worth knowing: one vertex per cell means a feature thinner than a
> voxel pinches. A subtraction's knife-edge rim leaves a handful of edges
> carrying four faces — watertight, but not manifold. `Detail` now distinguishes
> the two (`is_closed` / `is_manifold`), because voxelizing needs only the
> first and remeshing needs the second.
>
> Two of these were found by RENDERING rather than testing, which is now the
> third time this phase: a node whose arithmetic is right and whose wiring is
> never exercised looks exactly like a working node until you ask the viewport
> to draw it. Both new nodes now have resolver-level tests, not just unit tests
> on the field.
>
> **The 2D page context landed, and Phase 4 is done.** `src/page.rs` composes a
> printed sheet — inches, a DPI, straight-alpha RGBA — and four nodes build one:
> `page`, `page_grid`, `page_border`, `page_text`. It previews in the viewport's
> pane and exports a PNG whose pHYs chunk carries its physical size, so a
> printer lays the sheet out at the size it was composed at.
>
> It is a genuinely separate context, as proposed: page chains resolve through
> `resolve_page` and contribute nothing to the geometry the viewport draws.
> `export` is the only node in both, and what reaches it decides the format.
> `gem_graph` — the source family's everything-at-once node, 29 parameters — is
> deliberately not ported: it is the other four chained, which is the whole
> premise of the fifty-to-ten collapse.
>
> The blank-pane lesson: a new pane needs a `paint_widget` arm (the
> fall-through serves LEGACY widgets, so a modern-paint one draws nothing), a
> place in the `draw_order` sort (the viewport is full-bleed and panes float
> over it), and a line in the hand-listed roster test. Only the third fails
> loudly. Two shadow runs went into finding the first, and one of those was
> spent chasing a second designer process my kill had silently failed to stop —
> both were writing to one log, so I was reading one process's state against
> another's.
>
> Still outstanding: nothing in Phase 4.

Furthest out because it needs infrastructure nothing else does: a **volume
representation** (SDF or sparse grid) for shelling, offsetting and boolean work,
without which mold, sprue and support tooling has nothing to stand on. Then
**mesh export** — STL and OBJ, which the app cannot do at all today.

The COP family (`gem_page`, `gem_border`, `gem_grid`, `gem_text_box`,
`gem_graph`) is a second context entirely — 2D, printed output — and is best
treated as a separate surface rather than smuggled into the geometry graph.

The groundwork that already landed is the right groundwork: a declared world
unit, a scale readout and View 1:1 are precisely what a manufacturing tool needs
and what the Developer set does not care about.

Touches: a new `volume.rs`, a new `export.rs`, a 2D page context.

## Fifty operators, ten nodes

The HDA count is an artifact of Houdini's economics — a variant is cheaper as a
new asset than as a new parameter, so the families split and then had to be
merged back. Starting fresh, the merge is the starting point. The Scalar and
Vector families already collapsed into one in September 2026.

| Node in cce-designer | Absorbs | From |
|---|---|---|
| `attribute` | Attribute Initialize, Constant, Clip, Remap, Combine, Composite, Promote, Select, Normalize, Weight | 10 → 1 |
| | *(landed)* | |
| `neighbour` | Diffuse, Concentrate, Migrate, Bleed, Align, Lead, Charge — one Mode, one Neighbourhood | 7 → 1 |
| | *(landed; Charge is a proposed reading, not a port)* | |
| `gradient` | Gradient, Rotate, Direction | 3 → 1 |
| `analysis` | Analysis, Measure, Metamax, Time Analysis, Region Center | 5 → 1 |
| `time` | Time, Time Ramp, Time Switch | 3 → 1 |
| `develop` | Develop, Cull, Expire, Vitality, ID, Release | 6 → 1 |
| `remesh` | Surface Remesh, Surface Subdivide, Surface Adapt, Surface Open | 4 → 1 |
| `collide` | Surface Detangle, Surface Suture | 2 → 1 |
| `visualize` | Visualize, and the Solver's Vis tabs | 2 → 1 |
| `simnet` (exists) | Developer Solver, Submute Begin, Submute End | 3 → 0 |
| **Developer set** | **Nine new nodes, one already written** | **45 → 9** |

## Decisions to settle first

Four of these change what Phase 0 and Phase 1 look like, so they are worth
settling before the geometry model is written rather than after.

**1. Keep two kernel backends?** OpenCL plus a CPU interpreter means every ABI
change lands twice, and Phase 1 widens the ABI substantially. The interpreter
earns its keep as the semantic reference and keeps the suite green headless —
but it is a C-subset tree-walker with no vector types, and neighbour traversal
will strain it.
*Leaning:* keep both, but stop growing the kernel language — express
neighbourhood operators as native Rust evaluators and reserve kernels for
per-point math.

**2. Native nodes or editable templates?** Sphere, Plane and Extrude are subnet
templates whose kernel code the loader owns — a hand-edit inside an instance
reverts on load. Native nodes are Rust and not user-editable at all. The
Developer set could go either way, and which one decides whether a new operator
can be prototyped without a rebuild.
*Leaning:* native for anything touching topology; templates for the per-point
ops, so the experimentation surface stays open where it is cheap.

**3. How far does the attribute type system go?** Today: `Float` through
`Float4`. The Developer set needs integers (counters, ids, Vitality's ages) and
something dictionary-shaped — `Analysis` writes `<attr>_info` holding a range.
Adding integers is small; adding a dict type reaches into the spreadsheet, the
kernel ABI and serialization.
*Leaning:* integers and real groups in Phase 0. Replace the dictionary with
named detail attributes (`attr_min`, `attr_max`) unless there is a use for
nesting.

**4. Does existing work need to come across?** An HDA is VEX plus a SOP subnet;
neither has an equivalent here, so an importer would be a compiler for two
languages the app does not speak. If there are `.hip` scenes that must keep
running, that changes the priority order considerably.
*Leaning:* no importer. Rebuild the handful of setups worth keeping once the
vocabulary exists.

## What not to port

- **Houdini wrappers.** `developer_measure` is the Measure SOP with a promotion;
  `im_skeletonize`, `im_shortest_path` and the VDB nodes are thin skins over
  serious Houdini implementations. Each is a research project on its own — take
  them only where a chain actually needs one.
- **Dead on arrival.** `im_pose` and `im_sample` both failed to cook in
  `otls-audit.md`, and `developer_charge`, `im_bend`, `im_manipulator` and
  `im_scaffold` carry no read parameters at all. Do not carry forward what never
  worked. (`developer_charge` is the one exception taken so far: its name and
  its two parameter names were enough to design a mode around, and that mode is
  labelled a proposal in the code.)
- **Version forks.** Eleven operators ship as two or three live versions
  (`im_attractor` at 0.9, 1.0 and 1.1; `im_select` at 1.0 and 2.0). Port the
  newest, once.
- **The unbuilt list.** Energize, Edge Analysis, Analyze Change and Region were
  ideas without nodes in the first draft and still are. They belong in the
  design, not the port.

## Summary

Phase 0 is most of the risk and none of the fun. Phases 1 and 2 are where the
app starts doing something Houdini does not. Phase 6 is far enough out that it
should not influence any decision made now.

For a smaller first cut: Phase 0 plus the `neighbour` node alone is enough to
run a diffusion on a sphere and see it — which is the point at which the rest of
this becomes worth arguing about.
