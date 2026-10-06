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
//! They are edited in the dialog: [`crate::dialog::Mode::Visualizers`] lists
//! them with a switch each, and [`crate::dialog::Mode::VisualizerEdit`] is
//! one visualizer's settings as rows.

use crate::app::{FsNode, ParamDef, State};
use crate::detail::Detail;
use crate::dialog::{Control, Row};

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
    /// `Manual` over From..To — the node's Manual Range, a float2, kept as
    /// its two ends because the dialog edits each on a slider of its own
    /// (it has no float2 control) and the stored form names them so.
    pub range: String,
    pub from: f32,
    pub to: f32,
    pub blend: String,
    pub opacity: f32,
    /// A Vector line's length per unit of the attribute.
    pub scale: f32,
    /// A point group the visualizer is limited to; empty is every point.
    pub group: String,
}

impl Visualizer {
    /// A new visualizer on `attribute`, on, with the node's defaults.
    pub fn new(attribute: &str) -> Visualizer {
        Visualizer {
            enabled: true,
            attribute: attribute.to_string(),
            mode: ramp_mode(),
            ramp: viridis(),
            range: auto(),
            from: 0.0,
            to: 1.0,
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
                p("manual_range", "float2", format!("{}:{}", self.from, self.to)),
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
    for v in visualizers.iter().filter(|v| v.enabled && !v.attribute.trim().is_empty()) {
        let mut ignored = None;
        crate::geometry::apply_visualize(geom, &v.as_node(), &mut ignored);
    }
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
                ("from", v.from.to_string()),
                ("to", v.to.to_string()),
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
                    "from" => v.from = num(v.from),
                    "to" => v.to = num(v.to),
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

/// A visualizer in [`crate::dialog::Mode::Visualizers`]: the prefix, then
/// its index.
pub const VIS_ROW_PREFIX: &str = "vis:";
/// The list's Add Visualizer row.
pub const VIS_ADD_ROW_ID: &str = "vis:add";
/// A setting of the visualizer being edited, in
/// [`crate::dialog::Mode::VisualizerEdit`]: the prefix, then the field.
pub const VIS_FIELD_PREFIX: &str = "visfield:";

/// The Attribute and Group choices' word for "nothing chosen".
const NO_ATTRIBUTE: &str = "(none)";
const ALL_POINTS: &str = "(all points)";

fn choice(options: &[&str], current: &str) -> Control {
    let options: Vec<String> = options.iter().map(|s| s.to_string()).collect();
    let index = options.iter().position(|o| o.eq_ignore_ascii_case(current)).unwrap_or(0);
    Control::Choice { options, index }
}

fn row(field: &str, label: &str, control: Option<Control>) -> Row {
    Row { id: format!("{VIS_FIELD_PREFIX}{field}"), label: label.to_string(), chord: String::new(), control, truncate_head: false }
}

impl State {
    /// The list's rows: each visualizer, a switch each, and Add Visualizer.
    /// A visualizer whose attribute the scene does not have says so in the
    /// chord column, since it draws nothing.
    pub(crate) fn visualizer_rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .visualizers
            .iter()
            .enumerate()
            .map(|(i, v)| Row {
                id: format!("{VIS_ROW_PREFIX}{i}"),
                label: v.label(),
                chord: if self.scene_attributes.iter().any(|a| a.name == v.attribute) { String::new() } else { "not in the scene".to_string() },
                control: Some(Control::Toggle(v.enabled)),
                truncate_head: false,
            })
            .collect();
        rows.push(Row::plain(VIS_ADD_ROW_ID, "Add Visualizer", ""));
        rows
    }

    /// The rows of visualizer `i`'s settings, those that apply to its mode:
    /// Ramp's ramp, range and blend, Vector's scale — as the Visualize
    /// node's `show_when` conditions have them.
    pub(crate) fn visualizer_edit_rows(&self, i: usize) -> Vec<Row> {
        let Some(v) = self.visualizers.get(i) else { return Vec::new() };
        let mut rows = vec![row("enabled", "Enabled", Some(Control::Toggle(v.enabled)))];

        let mut attrs: Vec<String> = self.scene_attributes.iter().map(|a| a.name.clone()).collect();
        if !v.attribute.is_empty() && !attrs.contains(&v.attribute) {
            attrs.push(v.attribute.clone());
        }
        if attrs.is_empty() {
            attrs.push(NO_ATTRIBUTE.to_string());
        }
        let index = attrs.iter().position(|a| *a == v.attribute).unwrap_or(0);
        rows.push(row("attribute", "Attribute", Some(Control::Choice { options: attrs, index })));
        rows.push(row("mode", "Mode", Some(choice(&MODES, &v.mode))));
        if v.is_vector() {
            rows.push(row("scale", "Scale", Some(Control::Slider { value: v.scale, min: 0.0, max: 10.0f32.max(v.scale), dec: 2, step: 0.05, suffix: "" })));
        } else {
            rows.push(row("ramp", "Ramp", Some(choice(&RAMPS, &v.ramp))));
            rows.push(row("range", "Range", Some(choice(&RANGES, &v.range))));
            if v.is_manual() {
                let (lo, hi) = self.visualizer_value_range(v);
                let step = ((hi - lo) / 100.0).max(1e-4);
                rows.push(row("from", "From", Some(Control::Slider { value: v.from, min: lo, max: hi, dec: 3, step, suffix: "" })));
                rows.push(row("to", "To", Some(Control::Slider { value: v.to, min: lo, max: hi, dec: 3, step, suffix: "" })));
            }
            rows.push(row("blend", "Blend", Some(choice(&BLENDS, &v.blend))));
            rows.push(row("opacity", "Opacity", Some(Control::Slider { value: v.opacity, min: 0.0, max: 1.0, dec: 2, step: 0.05, suffix: "" })));
        }

        let mut groups: Vec<String> = vec![ALL_POINTS.to_string()];
        groups.extend(self.scene_groups.iter().map(|(g, _)| g.clone()));
        if !v.group.is_empty() && !groups.contains(&v.group) {
            groups.push(v.group.clone());
        }
        let index = if v.group.is_empty() { 0 } else { groups.iter().position(|g| *g == v.group).unwrap_or(0) };
        rows.push(row("group", "Group", Some(Control::Choice { options: groups, index })));
        rows.push(row("delete", "Delete Visualizer", None));
        rows.push(row("back", &format!("{} Back to Visualizers", crate::dialog::BACK_MARK), None));
        rows
    }

    /// The span a Manual range's sliders cover: the attribute's range in the
    /// scene with a quarter of it to spare each side, and the values in hand
    /// whatever they are.
    fn visualizer_value_range(&self, v: &Visualizer) -> (f32, f32) {
        let (mut lo, mut hi) = match self.scene_attributes.iter().find(|a| a.name == v.attribute) {
            Some(a) => {
                let pad = ((a.max - a.min) * 0.25).max(if a.max > a.min { 0.0 } else { 1.0 });
                (a.min - pad, a.max + pad)
            }
            None => (0.0, 1.0),
        };
        lo = lo.min(v.from).min(v.to);
        hi = hi.max(v.from).max(v.to);
        (lo, hi)
    }

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

    pub(crate) fn set_visualizer_enabled(&mut self, i: usize, on: bool) {
        if let Some(v) = self.visualizers.get_mut(i) {
            if v.enabled != on {
                v.enabled = on;
                self.visualizers_changed(true);
            }
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
            "from" => v.from = number().unwrap_or(v.from),
            "to" => v.to = number().unwrap_or(v.to),
            "opacity" => v.opacity = number().unwrap_or(v.opacity).clamp(0.0, 1.0),
            "scale" => v.scale = number().unwrap_or(v.scale).max(0.0),
            _ => return,
        }
        // Switching to Manual starts From and To at the attribute's own
        // range, which is what Auto was showing — not at 0..1.
        if field == "range" && v.is_manual() && v.from == 0.0 && v.to == 1.0 {
            if let Some(a) = self.scene_attributes.iter().find(|a| a.name == v.attribute) {
                (v.from, v.to) = (a.min, a.max);
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
