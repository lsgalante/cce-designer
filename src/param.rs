//! Node parameters: what a parameter holds, typed.
//!
//! A [`ParamDef`] keeps two things about its value and keeps them together:
//! the TEXT, exactly as the user or the file wrote it (`"0.50"` stays
//! `"0.50"`, so a save with nothing edited is byte-identical and the sim
//! cache keys that hash a simnet's JSON do not move), and the [`ParamSlot`]
//! that text parses to under the parameter's [`ParamKind`] — a number, a
//! vector, a switch, an option, or an expression still to be evaluated. Both
//! are private, and every setter re-parses, so the two cannot disagree: this
//! is phases 3 and 4 of the typed-value migration (CLAUDE.md, "Parameter
//! kinds").
//!
//! A text that does not fit its kind is kept verbatim as
//! [`ParamSlot::Invalid`] rather than dropped or coerced: readers then fall
//! back exactly as they did when every read parsed the string, the load says
//! how many values are affected, and the text survives for a person to fix.
//! What REFUSES a bad value is the entry points a person types into — the
//! params pane and MCP's `set_param` — via [`ParamDef::check`]; a load never
//! refuses anything.

use crate::app::FsNode;
use crate::expr::{fmt_num, Value};
use serde::{Deserialize, Serialize};

/// What a parameter HOLDS, parsed from its `type` string — the one place
/// that string is interpreted. The kind says how the text parses and which
/// control the params pane draws.
///
/// The head before the first `:` names the kind; what follows is the
/// kind's own detail (`slider:-2:2` a range, `choice:UV,Icosphere,Cube`
/// the options), read by the pane and by [`ParamDef::choice_options`]. A
/// type naming no kind is a template bug: `load_fs_tree` drops the template
/// and says so, and `every_shipped_template_param_has_a_known_kind` walks
/// the shipped ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Free text: a name, a path, a group or attribute name. `string` is
    /// the same kind — it is what an ABSENT type deserializes to.
    Text,
    /// A number with no range — a threshold, a scale factor, a manual
    /// ramp end — shown as a text row, because the pane's slider holds a
    /// fraction of its range and would clamp anything outside it.
    Float,
    /// A number over a range (`min`/`max`, or inline `slider:lo:hi`).
    Slider,
    /// A whole number, stepped.
    Spin,
    /// Three numbers, `x:y:z`.
    Float3,
    /// One of a fixed set of options, stored as the option's text.
    Choice,
    /// `true` / `false`.
    Toggle,
    /// A press, not a value: the pane writes `clicked` and the app clears it.
    Button,
    /// A program (a wrangle's script). Never an expression.
    Code,
    /// The NAME of another node, resolved sibling-first by
    /// `geometry::find_input_node` — an `Input` wire, a Boolean's `With`,
    /// a Relax's `Rest`. Empty means unconnected.
    Node,
    /// The NAME of a point attribute on the node's input — one it reads
    /// (Visualize's Attribute, Neighbour's Direction) or one it writes
    /// (Normal's Attribute, Suture's Counter). Any text is a valid value;
    /// what the kind changes is the pane, which offers the input's
    /// attributes as a picker on every row of this kind
    /// (`State::add_pick_lists`), where it used to know four rows by name.
    Attribute,
    /// The NAME of a point group on the node's input, read or written; the
    /// pane offers the input's groups. Empty means every point, which is
    /// what every Group row's default is.
    Group,
}

impl ParamKind {
    /// The type-string heads [`ParamKind::parse`] accepts, for messages.
    /// `string` is left out: it is an alias, not something to ask for.
    pub const NAMES: &'static [&'static str] =
        &["text", "float", "slider", "spinbox", "float3", "choice", "toggle", "button", "code", "node", "attribute", "group"];

    /// The kind's name — the type-string head that names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Float => "float",
            Self::Slider => "slider",
            Self::Spin => "spinbox",
            Self::Float3 => "float3",
            Self::Choice => "choice",
            Self::Toggle => "toggle",
            Self::Button => "button",
            Self::Code => "code",
            Self::Node => "node",
            Self::Attribute => "attribute",
            Self::Group => "group",
        }
    }

    /// The kind a `type` string names, or `None` when it names none.
    pub fn parse(ty: &str) -> Option<Self> {
        let head = ty.split(':').next().unwrap_or("").trim();
        Some(match head {
            "text" | "string" => Self::Text,
            "float" => Self::Float,
            "slider" => Self::Slider,
            "spinbox" => Self::Spin,
            "float3" => Self::Float3,
            "choice" => Self::Choice,
            "toggle" => Self::Toggle,
            "button" => Self::Button,
            "code" => Self::Code,
            "node" => Self::Node,
            "attribute" => Self::Attribute,
            "group" => Self::Group,
            _ => return None,
        })
    }
}

/// A parameter's value, parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// Float and Slider.
    Number(f32),
    /// Spin: a whole number.
    Int(i64),
    Vec3([f32; 3]),
    Bool(bool),
    /// The option, spelled as the options list spells it (the text may
    /// differ in case, and an empty text means the first option).
    Choice(String),
    /// Text, Node, Attribute, Group, Code and Button: the text is the value.
    Text(String),
}

impl ParamValue {
    /// The value as text. Numbers go through `expr::fmt_num` — the
    /// formatting every evaluated expression has always been written back
    /// with — so a typed write and the string write it replaced agree.
    pub fn to_text(&self) -> String {
        match self {
            ParamValue::Number(n) => fmt_num(*n as f64),
            ParamValue::Int(i) => i.to_string(),
            ParamValue::Vec3(v) => v.iter().map(|c| fmt_num(*c as f64)).collect::<Vec<_>>().join(":"),
            ParamValue::Bool(b) => b.to_string(),
            ParamValue::Choice(s) | ParamValue::Text(s) => s.clone(),
        }
    }
}

/// What a parameter's text parsed to.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamSlot {
    Value(ParamValue),
    /// An expression (`ch("../sphere1/Radius") * 2`), evaluated wherever the
    /// node is — the text is the expression. See `expr.rs`.
    Expr,
    /// A text that does not fit the kind, and why. Kept, never coerced.
    Invalid(String),
}

/// One node parameter: the template-owned UI metadata (label, type, range,
/// options, condition) and the instance-owned value.
#[derive(Clone)]
pub struct ParamDef {
    pub name: String,
    pub label: String,
    param_type: String,
    text: String,
    slot: ParamSlot,
    options: Vec<String>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    /// When this parameter should be SHOWN, as a condition over its siblings'
    /// current values. Empty means always.
    ///
    /// Grammar, deliberately tiny: `Mode == Twist`, `Mode == Twist|Bend` for
    /// any-of, `Mode != Bleed` for unless, and ` && ` between clauses. It
    /// exists because collapsing fifty operators into ten traded node count
    /// for parameter count — Attribute reached sixteen parameters, of which
    /// four matter at any moment — and a pane showing twelve irrelevant rows
    /// is worse than the twelve nodes it replaced.
    ///
    /// Houdini calls this `hideWhen`. Phrased the positive way round here
    /// because a template author is describing when a control APPLIES, and
    /// stating that directly is easier to get right than stating its negation.
    pub show_when: String,
    /// How a float3-valued row is SHOWN, where there is a choice:
    /// `trackball` for the ball beside the three sliders, `sliders` for
    /// the sliders alone, empty for the row's default
    /// ([`Self::wants_trackball`]). The row menu's Show / Hide Trackball
    /// writes it. It is the one piece of UI metadata the INSTANCE owns —
    /// a preference about a control, set by the person using it — so the
    /// template merge fills it only where the instance has not chosen.
    /// Serialized only when set, so a file that never used it is
    /// byte-identical to what it was.
    pub view: String,
    /// What the parameter does, in a sentence or two, shown in its row's
    /// right-click menu. The TEMPLATE's: `adopt_ui_from` hands it to every
    /// instance with the rest of the UI metadata, and it is read from a
    /// template file and never written — a save carries values, and a
    /// description kept there would go stale the day the template's
    /// wording changed (and move `sim_solve_key` the day it did).
    pub description: String,
    /// Which run of related parameters this one belongs to, by name — the
    /// params pane draws a separator wherever two rows it shows belong to
    /// different groups (`param_display`). The TEMPLATE's, read and never
    /// written, as the description is. Empty is the node's ungrouped run;
    /// the leading wires are a group of their own without saying so.
    pub group: String,
}

/// The file shape of a [`ParamDef`] — the struct as it was before the value
/// was typed, field for field and in the same order, so the JSON a save
/// writes (and `sim_solve_key` hashes) is unchanged. `expr` is serialized
/// only when set, as it always was.
#[derive(Clone, Deserialize, Serialize)]
struct ParamDefRepr {
    name: String,
    #[serde(default)]
    label: String,
    #[serde(rename = "type")]
    #[serde(default = "default_param_type")]
    param_type: String,
    #[serde(default)]
    default: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    min: Option<f32>,
    #[serde(default)]
    max: Option<f32>,
    #[serde(default)]
    step: Option<f32>,
    #[serde(default)]
    show_when: String,
    /// Whether `default` is an EXPRESSION to evaluate rather than a value.
    /// A flag and not a guess about the text, because a script contains
    /// `chf(`, a node name is an identifier and `0.5` parses as an
    /// expression too; Houdini makes the same choice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    expr: bool,
    /// [`ParamDef::view`]; absent unless one was chosen.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    view: String,
    /// [`ParamDef::description`]; read, never written.
    #[serde(default, skip_serializing)]
    description: String,
    /// [`ParamDef::group`]; read, never written.
    #[serde(default, skip_serializing)]
    group: String,
}

fn default_param_type() -> String {
    "string".to_string()
}

impl<'de> Deserialize<'de> for ParamDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = ParamDefRepr::deserialize(d)?;
        let mut p = ParamDef {
            name: r.name,
            label: r.label,
            param_type: r.param_type,
            text: r.default,
            slot: ParamSlot::Expr,
            options: r.options,
            min: r.min,
            max: r.max,
            step: r.step,
            show_when: r.show_when,
            view: r.view,
            description: r.description,
            group: r.group,
        };
        if !r.expr {
            p.reparse();
        }
        Ok(p)
    }
}

impl Serialize for ParamDef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ParamDefRepr {
            name: self.name.clone(),
            label: self.label.clone(),
            param_type: self.param_type.clone(),
            default: self.text.clone(),
            options: self.options.clone(),
            min: self.min,
            max: self.max,
            step: self.step,
            show_when: self.show_when.clone(),
            expr: self.is_expr(),
            view: self.view.clone(),
            description: String::new(),
            group: String::new(),
        }
        .serialize(s)
    }
}

/// `text` parsed under `kind`, with `options` the choices a Choice takes.
pub fn parse_value(kind: ParamKind, text: &str, options: &[String]) -> Result<ParamValue, String> {
    let t = text.trim();
    let number = |t: &str| t.parse::<f32>().ok().filter(|n| n.is_finite());
    match kind {
        ParamKind::Text | ParamKind::Node | ParamKind::Attribute | ParamKind::Group | ParamKind::Code | ParamKind::Button => {
            Ok(ParamValue::Text(text.to_string()))
        }
        ParamKind::Float | ParamKind::Slider => {
            number(t).map(ParamValue::Number).ok_or_else(|| format!("'{text}' is not a number"))
        }
        ParamKind::Spin => match t.parse::<f64>() {
            Ok(n) if n.is_finite() && n.fract() == 0.0 => Ok(ParamValue::Int(n as i64)),
            Ok(_) => Err(format!("'{text}' is not a whole number")),
            Err(_) => Err(format!("'{text}' is not a number")),
        },
        ParamKind::Float3 => {
            let parts: Vec<&str> = t.split(':').collect();
            match parts[..] {
                [x, y, z] => match (number(x.trim()), number(y.trim()), number(z.trim())) {
                    (Some(x), Some(y), Some(z)) => Ok(ParamValue::Vec3([x, y, z])),
                    _ => Err(format!("'{text}' is not three numbers")),
                },
                _ => Err(format!("'{text}' is not x:y:z")),
            }
        }
        ParamKind::Toggle => match t.to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => Ok(ParamValue::Bool(true)),
            "false" | "0" | "off" => Ok(ParamValue::Bool(false)),
            _ => Err(format!("'{text}' is not true or false")),
        },
        ParamKind::Choice => {
            if options.is_empty() {
                return Ok(ParamValue::Choice(text.to_string()));
            }
            if t.is_empty() {
                return Ok(ParamValue::Choice(options[0].clone()));
            }
            options
                .iter()
                .find(|o| o.eq_ignore_ascii_case(t))
                .map(|o| ParamValue::Choice(o.clone()))
                .ok_or_else(|| format!("'{text}' is not one of {}", options.join(", ")))
        }
    }
}

impl ParamDef {
    /// A parameter of type `ty` holding `text`, parsed. The rest of the
    /// metadata starts empty; the `with_*` builders fill it in.
    pub fn new(name: impl Into<String>, ty: impl Into<String>, text: impl Into<String>) -> Self {
        let mut p = ParamDef {
            name: name.into(),
            label: String::new(),
            param_type: ty.into(),
            text: text.into(),
            slot: ParamSlot::Expr,
            options: Vec::new(),
            min: None,
            max: None,
            step: None,
            show_when: String::new(),
            view: String::new(),
            description: String::new(),
            group: String::new(),
        };
        p.reparse();
        p
    }

    /// Whether a float3-valued row of this parameter shows the trackball:
    /// the instance's choice when it made one, `default` otherwise.
    pub fn wants_trackball(&self, default: bool) -> bool {
        match self.view.as_str() {
            "trackball" => true,
            "sliders" => false,
            _ => default,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_range(mut self, min: Option<f32>, max: Option<f32>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn with_step(mut self, step: Option<f32>) -> Self {
        self.step = step;
        self
    }

    pub fn with_options(mut self, options: Vec<String>) -> Self {
        self.options = options;
        self.reparse();
        self
    }

    pub fn with_show_when(mut self, cond: impl Into<String>) -> Self {
        self.show_when = cond.into();
        self
    }

    /// This parameter holding its text as an expression.
    pub fn as_expr(mut self) -> Self {
        self.set_expr(true);
        self
    }

    /// The value as written: the expression for an expression, the verbatim
    /// text otherwise.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The `type` string, detail and all (`slider:-2:2`).
    pub fn ty(&self) -> &str {
        &self.param_type
    }

    /// The options list the template gave (a Choice may carry its options
    /// in the type string instead — see [`ParamDef::choice_options`]).
    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn slot(&self) -> &ParamSlot {
        &self.slot
    }

    /// The parsed value, when the text is a value that fits the kind.
    pub fn value(&self) -> Option<&ParamValue> {
        match &self.slot {
            ParamSlot::Value(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_expr(&self) -> bool {
        self.slot == ParamSlot::Expr
    }

    /// Why the text does not fit the kind, when it does not.
    pub fn invalid(&self) -> Option<&str> {
        match &self.slot {
            ParamSlot::Invalid(why) => Some(why),
            _ => None,
        }
    }

    /// This parameter's kind. A type that names none reads as text — the
    /// row stays editable and its value survives — but a shipped template
    /// cannot carry one (see [`ParamKind`]).
    pub fn kind(&self) -> ParamKind {
        ParamKind::parse(&self.param_type).unwrap_or(ParamKind::Text)
    }

    /// A Choice's options: the list when there is one, else the ones the
    /// type string names (`choice:UV,Icosphere,Cube`).
    pub fn choice_options(&self) -> Vec<String> {
        if !self.options.is_empty() {
            self.options.clone()
        } else {
            self.param_type
                .strip_prefix("choice:")
                .map(|o| o.split(',').map(|x| x.trim().to_string()).collect())
                .unwrap_or_default()
        }
    }

    /// The range the pane holds this parameter to, as `(min, max, step)`,
    /// for the kinds that have one — Slider, Float3 and Spin. An inline
    /// detail (`slider:-2:2`) wins, then the template's `min` / `max`, then
    /// the pane's own defaults (0..2, -10..10, 1..10000), which are what a
    /// row with none declared clamps to. `param_display` builds the pane's
    /// row from this and the row menu's `Range:` reads it, so the two
    /// cannot disagree. `None` for a kind with no range.
    pub fn range(&self) -> Option<(f32, f32, Option<f32>)> {
        let (lo, hi) = match self.kind() {
            ParamKind::Slider => (0.0, 2.0),
            ParamKind::Float3 => (-10.0, 10.0),
            ParamKind::Spin => (1.0, 10000.0),
            _ => return None,
        };
        let (min, max, step) = self.declared_range();
        let step = match self.kind() {
            ParamKind::Spin => Some(step.unwrap_or(1.0)),
            _ => step,
        };
        Some((min.unwrap_or(lo), max.unwrap_or(hi), step))
    }

    /// The `(min, max, step)` the template DECLARES for this parameter —
    /// an inline `slider:-2:2` counts as declaring both ends — each `None`
    /// where it says nothing and the pane's default stands in
    /// ([`ParamDef::range`] is the result). What the row menu's `Min:` /
    /// `Max:` / `Step:` read, so a `none` there means the template left it
    /// to the pane.
    pub fn declared_range(&self) -> (Option<f32>, Option<f32>, Option<f32>) {
        let parts: Vec<&str> = self.param_type.split(':').collect();
        let inline = match parts[..] {
            [_, lo, hi] => lo.trim().parse::<f32>().ok().zip(hi.trim().parse::<f32>().ok()),
            _ => None,
        };
        match inline {
            Some((lo, hi)) => (Some(lo), Some(hi), self.step),
            None => (self.min, self.max, self.step),
        }
    }

    /// Whether a value that READS as a reference should become an
    /// expression here. Not for a code parameter: a kernel or a wrangle
    /// script is a program, and one whose whole text happens to be
    /// `ch("../a/Radius")` is a one-line program, not a channel — flagging
    /// it would evaluate the script to a number before it ever ran.
    pub fn takes_expressions(&self) -> bool {
        !(self.kind() == ParamKind::Code || self.name == "Code")
    }

    /// Whether `text` would be a valid VALUE here — what an entry point a
    /// person types into asks before it writes. Nothing is stored.
    pub fn check(&self, text: &str) -> Result<(), String> {
        parse_value(self.kind(), text, &self.choice_options()).map(|_| ())
    }

    /// Replace the text, keeping whether it is an expression, and re-parse.
    /// The one way a value changes: a text that does not fit is kept as
    /// [`ParamSlot::Invalid`], so a writer never loses what it wrote — the
    /// entry points that should refuse one ask [`ParamDef::check`] first.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        if !self.is_expr() {
            self.reparse();
        }
    }

    /// Store a value, as text formatted by [`ParamValue::to_text`]. Clears
    /// the expression flag: a value is not an expression.
    pub fn set_value(&mut self, v: ParamValue) {
        self.text = v.to_text();
        self.reparse();
    }

    /// A value written as text, clearing the expression flag — what an
    /// evaluated expression or Delete Expression leaves behind.
    pub fn bake(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.reparse();
    }

    /// Mark the text as an expression, or as a plain value again.
    pub fn set_expr(&mut self, on: bool) {
        if on {
            self.slot = ParamSlot::Expr;
        } else if self.is_expr() {
            self.reparse();
        }
    }

    /// Change the type, re-parsing the value under the new kind.
    pub fn set_type(&mut self, ty: impl Into<String>) {
        self.param_type = ty.into();
        if !self.is_expr() {
            self.reparse();
        }
    }

    /// Take the template's UI metadata — type, label, options, range, step,
    /// condition — keeping this instance's value, which is re-parsed under
    /// the (possibly new) kind. The template owns the surface, the instance
    /// owns its value.
    pub fn adopt_ui_from(&mut self, template: &ParamDef) {
        self.param_type = template.param_type.clone();
        self.label = template.label.clone();
        self.options = template.options.clone();
        self.min = template.min;
        self.max = template.max;
        self.step = template.step;
        self.show_when = template.show_when.clone();
        self.description = template.description.clone();
        self.group = template.group.clone();
        // The view is the instance's to choose; the template's is what it
        // starts from.
        if self.view.is_empty() {
            self.view = template.view.clone();
        }
        if !self.is_expr() {
            self.reparse();
        }
    }

    fn reparse(&mut self) {
        self.slot = match parse_value(self.kind(), &self.text, &self.choice_options()) {
            Ok(v) => ParamSlot::Value(v),
            Err(why) => ParamSlot::Invalid(why),
        };
    }

    /// An evaluated expression's result as the value this parameter holds —
    /// the row decides: a number into a toggle is its truth, into a choice
    /// the option at that index, into a spinbox the whole part; a string is
    /// parsed as the row's text would be. `None` when it fits nothing (a
    /// string into a slider that is not a number), which leaves the text
    /// the old write-back produced to be stored and flagged.
    pub fn value_from_expr(&self, v: &Value) -> Option<ParamValue> {
        let kind = self.kind();
        match (kind, v) {
            (ParamKind::Toggle, _) => Some(ParamValue::Bool(v.truthy())),
            (ParamKind::Choice, Value::Num(n)) => {
                let options = self.choice_options();
                if options.is_empty() {
                    Some(ParamValue::Choice(fmt_num(*n)))
                } else {
                    let i = (n.round().max(0.0) as usize).min(options.len() - 1);
                    Some(ParamValue::Choice(options[i].clone()))
                }
            }
            (ParamKind::Spin, Value::Num(n)) if n.is_finite() => Some(ParamValue::Int(n.trunc() as i64)),
            (ParamKind::Float | ParamKind::Slider, Value::Num(n)) => Some(ParamValue::Number(*n as f32)),
            _ => parse_value(kind, &v.as_str(), &self.choice_options()).ok(),
        }
    }
}

/// Every parameter in `node`'s tree whose type names no [`ParamKind`], as
/// `(path, parameter, type)`. Empty for a well-formed template.
pub fn unknown_param_kinds(node: &FsNode) -> Vec<(String, String, String)> {
    fn walk(node: &FsNode, path: &str, out: &mut Vec<(String, String, String)>) {
        for p in &node.params {
            if ParamKind::parse(&p.param_type).is_none() {
                out.push((path.to_string(), p.name.clone(), p.param_type.clone()));
            }
        }
        for c in &node.children {
            walk(c, &format!("{path}/{}", c.name), out);
        }
    }
    let mut out = Vec::new();
    walk(node, &node.name, &mut out);
    out
}

/// Every parameter in `node`'s tree whose text does not fit its kind, as
/// `(path, parameter, why)` — what a load reports on the status line.
pub fn invalid_params(node: &FsNode) -> Vec<(String, String, String)> {
    fn walk(node: &FsNode, path: &str, out: &mut Vec<(String, String, String)>) {
        for p in &node.params {
            if let Some(why) = p.invalid() {
                out.push((path.to_string(), p.name.clone(), why.to_string()));
            }
        }
        for c in &node.children {
            walk(c, &format!("{path}/{}", c.name), out);
        }
    }
    let mut out = Vec::new();
    for c in &node.children {
        walk(c, &c.name, &mut out);
    }
    out
}
