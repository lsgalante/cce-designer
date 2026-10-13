//! `State::params`: the params pane's own state beside its widget — which
//! node its rows were loaded from, its plate (on or off, as drawn, the
//! HUD's width), the spans its value and range rows run over, the picker
//! lists read off the node's input, the visualizers taking the HUD over,
//! and Copy Parameter's clipboard.

use super::{ParamPaneTarget, PickLists};

pub struct ParamsPane {
    /// The node the params pane's rows were loaded FROM — set by
    /// `sync_parameters_pane`, checked by `sync_parameters_to_project`, which
    /// writes nothing when the selection has moved on since. See
    /// [`State::param_pane_target`](crate::app::State::param_pane_target).
    pub(crate) param_pane_source: Option<ParamPaneTarget>,
    /// Whether the params pane draws its PLATE. With it off the rows stand
    /// directly over the scene, and the pane claims only what its rows
    /// cover (`params_claim`), so the scene under the rest of its rect
    /// orbits, takes the wheel and shows its point numbers.
    pub params_plate: bool,
    /// The params plate as drawn: `[x, y, w, h, round]`, eased each tick
    /// toward `params_plate_target` — `round` 0 is the plate fitted to the
    /// rows, 1 the small circle it collapses to with no rows to show. `None`
    /// with no plate, and before the first tick (the target is drawn then).
    pub params_plate_shown: Option<[f32; 5]>,
    /// The params HUD's width as asked for (`params_hud_rect` fits it to
    /// the viewport). It lives on the scene, not in a dock — there are none
    /// (since 2026-10-07).
    pub params_hud_width: f32,
    /// The half-span the Attribute node's Value row runs over, with the
    /// node it is for (`value_row_span`): kept between pane syncs so the row
    /// re-scales only when its value leaves it.
    pub value_row_span: Option<(String, f32)>,
    /// The same for the shown node's `float2` rows, by parameter name, with
    /// the node they are for (`present_float2_rows`).
    pub float2_spans: Option<(String, Vec<(String, f32)>)>,
    /// The param pane's completion lists — (input node name, geometry
    /// version) → (group names, attribute names) read off that input's
    /// evaluated geometry, feeding the textpick rows on group/attribute
    /// params. One entry: the selected node's input.
    pub pick_cache: Option<((String, u64), PickLists)>,
    /// The params HUD shows the attribute visualizers, editing visualizer
    /// `i` (0 with none yet), in place of the selected node's parameters —
    /// `visualizer::State::open_visualizers_hud`. `None` is the node.
    pub vis_hud: Option<usize>,
    /// What the HUD would have shown when the visualizers took it: picking
    /// another node hands the HUD back to that node's parameters.
    pub(crate) vis_hud_from: Option<ParamPaneTarget>,
    /// Copy Parameter's clipboard: (node id, parameter name). An id, so a
    /// rename between the copy and the paste still pastes the right path.
    pub copied_param: Option<(String, String)>,
}
