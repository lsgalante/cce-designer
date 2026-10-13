//! `State::scene`: the displayed scene as the app last built it — the
//! evaluated geometry before the visualizers, its attributes and groups,
//! the fill and edge vertices the views draw it with, its light, the image
//! a page level shows, and the simulation cache every evaluation shares.
//! What is staged FROM it for the renderer is `State::stage`'s; what is
//! drawn on it, `State::markers`'.

use super::Vertex3D;

pub struct SceneData {
    /// Sphere-geometry cache for the path tracer (a copy of the last
    /// `rebuild_scene_geometry` output, so entering RT mode never re-runs
    /// the node graph).
    pub rt_sphere_verts: Vec<Vertex3D>,
    /// Bumped by `rebuild_scene_geometry`; part of the RT-scene cache key.
    pub rt_geometry_version: u64,
    /// The scene fill with smooth shading baked into its colours
    /// (`geometry::smooth_lit_vertices`), built by `rebuild_scene_geometry`
    /// while `smooth_shading` is on and empty otherwise. The raster pass
    /// uploads this in place of `rt_sphere_verts`, which the path tracer
    /// keeps reading unlit.
    pub scene_smooth_verts: Vec<Vertex3D>,
    /// The visible scene's own edges for the wire pass (LINE_LIST pairs),
    /// rebuilt with the scene while Show Wireframe is on and empty while it
    /// is off. Topological — see `render::scene_edge_verts`.
    pub scene_edge_verts: Vec<Vertex3D>,
    /// The displayed scene as last EVALUATED, before the visualizers: what a
    /// visualizer edit re-presents (`State::revisualize`) without running
    /// the graph again.
    pub scene_base: Option<crate::detail::Detail>,
    /// The displayed scene's point attributes as last built, with their
    /// ranges: what the visualizer editor offers.
    pub scene_attributes: Vec<crate::visualizer::SceneAttribute>,
    /// Every point group of the scene as last built, with its members'
    /// positions: what the dialog lists and what the markers are built
    /// from, so a switch flipped evaluates nothing.
    pub scene_groups: Vec<(String, Vec<[f32; 3]>)>,
    /// The scene's light, as the Environment node at the root says (or its
    /// defaults) — read by `present_scene` and `sync_environment`, handed
    /// to both views by the stage pass. See `crate::environment`.
    pub environment: crate::environment::Environment,
    /// The GPU image of the page the viewport shows. Owned here, so
    /// replacing a page frees the one it replaces.
    pub page_image: Option<u32>,
    /// The page that image is of: what the scene pass places it by and the
    /// framing commands fit the camera to. None when the level shows none.
    pub page_shown: Option<crate::page::PageShown>,
    /// What the page on show was composed from ([`crate::page::chain_key`])
    /// and the status line that announced it. A geometry rebuild — any edit
    /// at the level, every frame of a playing sim — recomposes the page, and
    /// a Letter sheet at 300 DPI is ~90 ms of compose and conversion plus a
    /// 34 MB upload; with this the ones that change nothing it reads cost a
    /// hash of its chain.
    pub page_composed: Option<(u64, String)>,
    /// Set when the page raster must be re-uploaded — after a replacement
    /// renderer drops the old id. Consumed on the next tick rather than acted
    /// on in `renderer_init`, which runs before the frame has settled and
    /// where relaying the panes would be premature.
    pub page_dirty: bool,
    /// Counts the recompositions of the shown image, as
    /// `rt_geometry_version` counts the geometry's.
    pub page_version: u64,
    /// Solved simulation states, kept across frames so playing forward costs one
    /// step per frame instead of re-solving from the start frame every redraw.
    pub sim_cache: crate::geometry::SimCache,
    /// Frame the scene was last built at, so the timeline moving can invalidate it.
    pub last_sim_frame: i32,
    /// What the playbar's cache strip was last worked out from: the sim
    /// cache's revision, the geometry version (an edit moves it, and may
    /// make what is cached stale without solving anything) and the frame
    /// range. See [`State::sync_playbar_cache`](crate::app::State::sync_playbar_cache).
    pub playbar_cache_key: Option<(u64, u64, i32, i32)>,
}
