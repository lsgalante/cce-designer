//! Attribute visualizers: Houdini's viewport visualizers. A visualizer shows
//! a point attribute of whatever the viewport displays — coloured through a
//! ramp, or drawn as a line from each point — without a node in the graph.
//!
//! It is the Visualize NODE's reading of an attribute, and runs through that
//! node's own [`crate::geometry::apply_visualize`] over a node built from its
//! settings ([`Visualizer::as_node`]), so the two cannot disagree about what
//! a Ramp or a Vector draws. What differs is where it lives: a node is part
//! of the scene and travels with its chain; a visualizer is a DISPLAY
//! setting, as every display setting here is — a live field on `State`
//! (`State::visualizers`), persisted to state.kdl and with the project's
//! display block, applied to the merged scene `Detail` at the end of a scene
//! rebuild (`State::present_scene`), and reached from the palette (the
//! `attribute_visualizers` command). Several apply in order, the later over
//! the earlier, as a chain of Visualize nodes composites.
//!
//! They are edited in the params HUD (`State::vis_hud`, since 2026-10-06;
//! they were the dialog's Visualizers / VisualizerEdit modes): a Visualizer
//! dropdown picks the one edited, Add and Delete beside it, its settings as
//! ordinary rows under it, and Done back to the selected node. The rows are
//! a pseudo-node's parameters (`visualizer_hud_params`), so the HUD's own
//! row building, `show_when` and controls serve them unchanged.

use crate::app::{FsNode, ParamDef, State};
use crate::detail::Detail;

pub const MODES: [&str; 2] = ["Ramp", "Vector"];
pub const RAMPS: [&str; 4] = ["Grayscale", "Heat", "Spectrum", "Viridis"];
pub const RANGES: [&str; 2] = ["Auto", "Manual"];
pub const BLENDS: [&str; 3] = ["Set", "Multiply", "Add"];

fn ramp_mode() -> String {
    "Ramp".into()
}
fn viridis() -> String {
    "Viridis".into()
}
fn auto() -> String {
    "Auto".into()
}
fn set() -> String {
    "Set".into()
}
fn fifth() -> f32 {
    0.2
}

/// One visualizer: the Visualize node's settings, under the same names and
/// options, and whether it is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Visualizer {
    pub enabled: bool,
    /// The point attribute shown.
    pub attribute: String,
    /// `Ramp` colours points by the attribute; `Vector` draws a line along
    /// it from each point.
    pub mode: String,
    pub ramp: String,
    /// `Auto` spreads the ramp over the attribute's range in the scene;
    /// `Manual` over Manual Range.
    pub range: String,
    /// The ramp's two ends under a Manual range: the node's float2, edited
    /// in the params HUD as one `float2` row.
    pub manual_range: [f32; 2],
    pub blend: String,
    pub opacity: f32,
    /// A Vector line's length per unit of the attribute.
    pub scale: f32,
    /// A point group the visualizer is limited to; empty is every point.
    pub group: String,
}

impl Visualizer {
    /// Whether this one does anything: on, and naming an attribute.
    pub fn applies(&self) -> bool {
        self.enabled && !self.attribute.trim().is_empty()
    }

    /// A new visualizer on `attribute`, on, with the node's defaults.
    pub fn new(attribute: &str) -> Visualizer {
        Visualizer {
            enabled: true,
            attribute: attribute.to_string(),
            mode: ramp_mode(),
            ramp: viridis(),
            range: auto(),
            manual_range: [0.0, 1.0],
            blend: set(),
            opacity: 1.0,
            scale: fifth(),
            group: String::new(),
        }
    }

    pub fn is_vector(&self) -> bool {
        self.mode.eq_ignore_ascii_case("vector")
    }

    pub fn is_manual(&self) -> bool {
        self.range.eq_ignore_ascii_case("manual")
    }

    /// What the list calls it: the attribute and how it is shown.
    pub fn label(&self) -> String {
        let attr = if self.attribute.is_empty() { "(no attribute)" } else { &self.attribute };
        let how = if self.is_vector() { format!("Vector ×{:.2}", self.scale) } else { format!("Ramp, {}", self.ramp) };
        let group = if self.group.is_empty() { String::new() } else { format!(" in {}", self.group) };
        format!("{attr} — {how}{group}")
    }

    /// A Visualize node carrying these settings, for `apply_visualize`.
    pub fn as_node(&self) -> FsNode {
        let p = |name: &str, ty: &str, text: String| ParamDef::new(name.to_string(), ty.to_string(), text);
        FsNode {
            id: String::new(),
            name: "visualizer".to_string(),
            node_type: "visualize".to_string(),
            children: Vec::new(),
            params: vec![
                p("attribute", "attribute", self.attribute.clone()),
                p("mode", "choice", self.mode.clone()),
                p("ramp", "choice", self.ramp.clone()),
                p("range", "choice", self.range.clone()),
                p("manual_range", "float2", format!("{}:{}", self.manual_range[0], self.manual_range[1])),
                p("blend", "choice", self.blend.clone()),
                p("opacity", "float", self.opacity.to_string()),
                p("scale", "float", self.scale.to_string()),
                p("group", "group", self.group.clone()),
            ],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        }
    }
}

/// Apply every visualizer that is on to `geom`, in order. One naming an
/// attribute the scene does not have does nothing: it is a display setting,
/// and the scene it was made for may come back.
pub fn apply_all(visualizers: &[Visualizer], geom: &mut Detail) {
    for v in visualizers.iter().filter(|v| v.applies()) {
        let mut ignored = None;
        crate::geometry::apply_visualize(geom, &v.as_node(), &mut ignored);
    }
}

/// Whether [`apply_all`] would change anything: some visualizer is on and
/// names an attribute.
pub fn any_applies(visualizers: &[Visualizer]) -> bool {
    visualizers.iter().any(Visualizer::applies)
}

/// The visualizers as the settings hold them: one string, a visualizer per
/// `;`, each `key=value` pairs joined by `|`, with `%`, `|`, `;`, `=`, `"`
/// and `\` percent-escaped in the values. One string, as the marked groups
/// are one, because the KDL writer cannot be trusted with a list of
/// records; and not JSON, because the writer then put a string between
/// quotes WITHOUT escaping the ones inside it — a JSON string came back as
/// a line no parser reads, and a settings file that fails to parse is read
/// as the DEFAULTS. (cce-ui escapes since 2026-10-01; this format stays,
/// being what the files hold.) A key the reader does not know is skipped
/// and one it lacks takes the default, so the format can grow.
pub fn encode(visualizers: &[Visualizer]) -> String {
    visualizers
        .iter()
        .map(|v| {
            [
                ("enabled", v.enabled.to_string()),
                ("attribute", v.attribute.clone()),
                ("mode", v.mode.clone()),
                ("ramp", v.ramp.clone()),
                ("range", v.range.clone()),
                ("manual_range", format!("{}:{}", v.manual_range[0], v.manual_range[1])),
                ("blend", v.blend.clone()),
                ("opacity", v.opacity.to_string()),
                ("scale", v.scale.to_string()),
                ("group", v.group.clone()),
            ]
            .iter()
            .map(|(k, val)| format!("{k}={}", escape(val)))
            .collect::<Vec<_>>()
            .join("|")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// `lo:hi` as two numbers.
fn two(text: &str) -> Option<[f32; 2]> {
    let (a, b) = text.split_once(':')?;
    Some([a.trim().parse().ok()?, b.trim().parse().ok()?])
}

/// The visualizers out of the settings' one string; a record that names no
/// field it knows is dropped.
pub fn decode(text: &str) -> Vec<Visualizer> {
    text.split(';')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|record| {
            let mut v = Visualizer::new("");
            let mut known = false;
            for pair in record.split('|') {
                let Some((k, raw)) = pair.split_once('=') else { continue };
                let val = unescape(raw);
                let num = |d: f32| val.parse::<f32>().unwrap_or(d);
                known = true;
                match k.trim() {
                    "enabled" => v.enabled = val != "false",
                    "attribute" => v.attribute = val,
                    "mode" => v.mode = val,
                    "ramp" => v.ramp = val,
                    "range" => v.range = val,
                    "manual_range" => v.manual_range = two(&val).unwrap_or(v.manual_range),
                    // Its two ends, as a settings file from before kept them.
                    "from" => v.manual_range[0] = num(v.manual_range[0]),
                    "to" => v.manual_range[1] = num(v.manual_range[1]),
                    "blend" => v.blend = val,
                    "opacity" => v.opacity = num(v.opacity),
                    "scale" => v.scale = num(v.scale),
                    "group" => v.group = val,
                    _ => {}
                }
            }
            known.then_some(v)
        })
        .collect()
}

const ESCAPED: [char; 6] = ['%', '|', ';', '=', '"', '\\'];

fn escape(s: &str) -> String {
    s.chars().map(|c| if ESCAPED.contains(&c) { format!("%{:02X}", c as u32) } else { c.to_string() }).collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                Some(d) => out.push(d),
                None => {
                    out.push('%');
                    out.push_str(&hex);
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A point attribute of the displayed scene, as the editor offers it: its
/// name and the range of its first component, which is what a Ramp reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneAttribute {
    pub name: String,
    pub min: f32,
    pub max: f32,
}

/// The scene's point attributes, by name, with their ranges — leaving out the
/// vector markers a Visualize stages (`vis_*`), which are not the scene's.
pub fn scene_attributes(geom: &Detail) -> Vec<SceneAttribute> {
    let mut out: Vec<SceneAttribute> = geom
        .points()
        .names()
        .into_iter()
        .filter(|n| !n.starts_with(crate::detail::VIS_PREFIX))
        .map(|name| {
            let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
            for p in 0..geom.num_points() {
                if let Some(v) = geom.points().value(name, p) {
                    let x = v.as_f32();
                    if x.is_finite() {
                        min = min.min(x);
                        max = max.max(x);
                    }
                }
            }
            if !min.is_finite() {
                (min, max) = (0.0, 1.0);
            }
            SceneAttribute { name: name.to_string(), min, max }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The Attribute and Group choices' word for "nothing chosen".
const NO_ATTRIBUTE: &str = "(none)";
const ALL_POINTS: &str = "(all points)";

impl State {
    /// Add a visualizer, on, on the first attribute of the scene that is
    /// not its colour or its position — what is worth looking at — and
    /// return its index.
    pub(crate) fn add_visualizer(&mut self) -> usize {
        let pick = self
            .scene_attributes
            .iter()
            .map(|a| a.name.as_str())
            .find(|n| !matches!(*n, "P" | "Cd" | "N"))
            .or_else(|| self.scene_attributes.first().map(|a| a.name.as_str()))
            .unwrap_or("")
            .to_string();
        self.visualizers.push(Visualizer::new(&pick));
        self.visualizers_changed(true);
        self.visualizers.len() - 1
    }

    pub(crate) fn delete_visualizer(&mut self, i: usize) {
        if i < self.visualizers.len() {
            self.visualizers.remove(i);
            self.visualizers_changed(true);
        }
    }

    /// Write one of visualizer `i`'s settings from the text its row holds —
    /// a choice's option, a toggle's `true`/`false`, a number.
    pub(crate) fn set_visualizer_field(&mut self, i: usize, field: &str, value: &str, save: bool) {
        let Some(v) = self.visualizers.get_mut(i) else { return };
        let number = || value.parse::<f32>().ok();
        match field {
            "enabled" => v.enabled = value == "true",
            "attribute" => v.attribute = if value == NO_ATTRIBUTE { String::new() } else { value.to_string() },
            "mode" => v.mode = value.to_string(),
            "ramp" => v.ramp = value.to_string(),
            "range" => v.range = value.to_string(),
            "blend" => v.blend = value.to_string(),
            "group" => v.group = if value == ALL_POINTS { String::new() } else { value.to_string() },
            "manual_range" => v.manual_range = two(value).unwrap_or(v.manual_range),
            "opacity" => v.opacity = number().unwrap_or(v.opacity).clamp(0.0, 1.0),
            "scale" => v.scale = number().unwrap_or(v.scale).max(0.0),
            _ => return,
        }
        // Switching to Manual starts the range at the attribute's own,
        // which is what Auto was showing — not at 0..1.
        if field == "range" && v.is_manual() && v.manual_range == [0.0, 1.0] {
            if let Some(a) = self.scene_attributes.iter().find(|a| a.name == v.attribute) {
                v.manual_range = [a.min, a.max];
            }
        }
        self.visualizers_changed(save);
    }

    /// Show the scene under the visualizers as they now stand, and keep them.
    fn visualizers_changed(&mut self, save: bool) {
        self.revisualize();
        if save {
            self.save_settings();
        }
    }
}

/// The HUD's visualizer view: its rows' parameter names.
const HUD_PICK: &str = "visualizer";
const HUD_ADD: &str = "add_visualizer";
const HUD_DELETE: &str = "delete_visualizer";
const HUD_DONE: &str = "done";

impl State {
    /// Show the attribute visualizers in the params HUD, editing the first
    /// (or `i`), in place of the selected node's parameters.
    pub(crate) fn open_visualizers_hud(&mut self) {
        self.vis_hud = Some(self.vis_hud.unwrap_or(0).min(self.visualizers.len().saturating_sub(1)));
        self.vis_hud_from = self.param_pane_target();
        if !self.show_parameters {
            self.execute_menu_action("Show Parameters Pane");
        }
        self.sync_parameters_pane();
        self.update_status_text("Attribute Visualizers: in the parameters; Done goes back to the node.");
    }

    /// Back to the selected node's parameters.
    pub(crate) fn close_visualizers_hud(&mut self) {
        if self.vis_hud.take().is_some() {
            self.sync_parameters_pane();
        }
    }

    /// What the picker calls visualizer `i`.
    fn visualizer_pick_label(&self, i: usize) -> String {
        let v = &self.visualizers[i];
        let attr = if v.attribute.is_empty() { NO_ATTRIBUTE } else { &v.attribute };
        format!("#{} {attr}", i + 1)
    }

    /// The HUD's visualizer view as a pseudo-node's parameters: the picker,
    /// Add and Delete, the edited visualizer's settings under the Visualize
    /// node's names and `show_when` conditions, and Done. With none yet,
    /// Add and Done alone.
    pub(crate) fn visualizer_hud_params(&self) -> Vec<ParamDef> {
        let p = |name: &str, ty: String, text: String, label: &str| ParamDef::new(name.to_string(), ty, text).with_label(label);
        let grouped = |mut d: ParamDef, g: &str| {
            d.group = g.to_string();
            d
        };
        let mut out = Vec::new();
        let editing = self.vis_hud.filter(|&i| i < self.visualizers.len());
        if let Some(i) = editing {
            let labels: Vec<String> = (0..self.visualizers.len()).map(|k| self.visualizer_pick_label(k)).collect();
            out.push(grouped(p(HUD_PICK, format!("choice:{}", labels.join(",")), labels[i].clone(), "Visualizer"), "pick"));
        }
        out.push(grouped(p(HUD_ADD, "button".into(), String::new(), "Add Visualizer"), "pick"));
        if let Some(i) = editing {
            out.push(grouped(p(HUD_DELETE, "button".into(), String::new(), "Delete Visualizer"), "pick"));
            let v = &self.visualizers[i];
            out.push(grouped(p("enabled", "toggle".into(), v.enabled.to_string(), "Enabled"), "settings"));
            let mut attrs: Vec<String> = self.scene_attributes.iter().map(|a| a.name.clone()).collect();
            if !v.attribute.is_empty() && !attrs.contains(&v.attribute) {
                attrs.push(v.attribute.clone());
            }
            if attrs.is_empty() {
                attrs.push(NO_ATTRIBUTE.to_string());
            }
            let attr = if v.attribute.is_empty() { attrs[0].clone() } else { v.attribute.clone() };
            out.push(grouped(p("attribute", format!("choice:{}", attrs.join(",")), attr, "Attribute"), "settings"));
            out.push(grouped(p("mode", format!("choice:{}", MODES.join(",")), v.mode.clone(), "Mode"), "settings"));
            out.push(grouped(p("ramp", format!("choice:{}", RAMPS.join(",")), v.ramp.clone(), "Ramp").with_show_when("mode == Ramp"), "settings"));
            out.push(grouped(p("range", format!("choice:{}", RANGES.join(",")), v.range.clone(), "Range").with_show_when("mode == Ramp"), "settings"));
            out.push(grouped(
                p("manual_range", "float2".into(), format!("{}:{}", v.manual_range[0], v.manual_range[1]), "Manual Range").with_show_when("mode == Ramp && range == Manual"),
                "settings",
            ));
            out.push(grouped(p("blend", format!("choice:{}", BLENDS.join(",")), v.blend.clone(), "Blend").with_show_when("mode == Ramp"), "settings"));
            out.push(grouped(p("opacity", "slider:0:1".into(), format!("{:.2}", v.opacity), "Opacity").with_show_when("mode == Ramp"), "settings"));
            out.push(grouped(p("scale", format!("slider:0:{}", 10.0f32.max(v.scale)), format!("{:.2}", v.scale), "Scale").with_show_when("mode == Vector"), "settings"));
            let mut groups: Vec<String> = vec![ALL_POINTS.to_string()];
            groups.extend(self.scene_groups.iter().map(|(g, _)| g.clone()));
            if !v.group.is_empty() && !groups.contains(&v.group) {
                groups.push(v.group.clone());
            }
            let group = if v.group.is_empty() { ALL_POINTS.to_string() } else { v.group.clone() };
            out.push(grouped(p("group", format!("choice:{}", groups.join(",")), group, "Group"), "where"));
        }
        out.push(grouped(p(HUD_DONE, "button".into(), String::new(), "Done"), "done"));
        out
    }

    /// The HUD's rows written back into the visualizers: each row whose
    /// value differs from what the view shows becomes the edit it names.
    /// A change that adds or drops rows (another visualizer, Mode, Range,
    /// Add, Delete) re-reads the view; a slider being dragged does not,
    /// which would drop the slider held.
    pub(crate) fn sync_visualizer_hud_back(&mut self) {
        let shown = self.visualizer_hud_params();
        let rows = self.param().node_params();
        let mut reread = false;
        let mut changed = false;
        for (key, value, _) in rows {
            let Some(def) = shown.iter().find(|d| d.shown_name() == key) else { continue };
            if def.text() == value {
                continue;
            }
            let i = self.vis_hud.unwrap_or(0);
            match def.name.as_str() {
                HUD_PICK => {
                    if let Some(k) = (0..self.visualizers.len()).find(|&k| self.visualizer_pick_label(k) == value) {
                        self.vis_hud = Some(k);
                    }
                    reread = true;
                }
                HUD_ADD if value == "clicked" => {
                    let k = self.add_visualizer();
                    self.vis_hud = Some(k);
                    reread = true;
                }
                HUD_DELETE if value == "clicked" => {
                    self.delete_visualizer(i);
                    self.vis_hud = Some(i.min(self.visualizers.len().saturating_sub(1)));
                    reread = true;
                }
                HUD_DONE if value == "clicked" => {
                    self.close_visualizers_hud();
                    return;
                }
                HUD_ADD | HUD_DELETE | HUD_DONE => {}
                field => {
                    // Saved at the frame, not per motion of a drag.
                    self.set_visualizer_field(i, field, &value, false);
                    changed = true;
                    if matches!(field, "mode" | "range" | "attribute") {
                        reread = true;
                    }
                }
            }
        }
        if changed {
            self.settings_save_pending = true;
        }
        if reread {
            self.sync_parameters_pane();
        }
    }
}
