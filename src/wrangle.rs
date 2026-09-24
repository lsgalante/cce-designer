//! The wrangle node: a script run once per element, over the Detail's own
//! surface.
//!
//! Houdini's attribwrangle, on an engine someone else maintains. The script
//! language is Rhai, chosen in `shapeshifter.md` Phase 7 for being pure Rust
//! with no C toolchain, sandboxed behind an operation budget — the step budget
//! the retired `kernel_cpu` interpreter used to count by hand — and compiled
//! once to an AST that is cached by source, as the retired launcher cached
//! kernels. What this module adds is the BINDING: how a script reaches the
//! geometry, and nothing else.
//!
//! The vocabulary, deliberately VEX-shaped so it reads as it does there:
//!
//! - `@P`, `@Cd`, `@N`, `@id`, `@ptnum` / `@primnum`, `@numpt` / `@numprim`,
//!   `@Frame`, and `@name` for any attribute of any type. `@` is sugar —
//!   Rhai has no such token — rewritten by [`desugar`] into an index on the
//!   element (`__at["name"]`), so `@mass += 2.0` and `@P.y = 0.0` are ordinary
//!   Rhai once the script reaches the engine. Naming an attribute creates it,
//!   typed by the value written: a float, an int, a `vec3`, an array of two
//!   or four. A float2 reads as a `vec3` with z = 0 and a float4 as an array.
//! - `ch("path")`, `chs`, `chv`, `chi` — the node's own parameters and, by
//!   Houdini's relative paths, any other node's. The paths are resolved BEFORE
//!   the script runs, through the expression scope, so an expression-valued
//!   parameter is evaluated first and the script sees its value; the cost per
//!   element is a map lookup. A path therefore has to be a string literal.
//! - `neighbours(pt)`, `prims(pt)`, `points(prim)` off the derived topology —
//!   what decision 1 kept out of the kernel language because the interpreter
//!   could not carry it — and `nearest(pos, radius)` off the point grid.
//! - `point(name, i)` / `setpoint`, `prim(name, i)` / `setprim`,
//!   `detail(name)` / `setdetail`, `ingroup(name)` / `setgroup(name, bool)`.
//! - `addpoint(pos)`, `addprim([a, b, c])`, `removepoint(i)` — DEFERRED and
//!   applied after the run, so a script iterating points sees a stable count.
//!   `addpoint` returns the index the point will have.
//! - `vec3(x, y, z)`, `dot`, `cross`, `length`, `normalize`, `distance`,
//!   `lerp`, `fit`, `clamp`, `rand(seed)`, and Rhai's own math.
//!
//! CPU only, and deliberately: an interpreter is an order of magnitude or
//! more below native Rust, which is fine for a wrangle over tens of thousands
//! of elements per edit and wrong for a solver at a million per frame — that
//! is Phase 7's step 4, WGSL compute through the renderer, and not this.

use crate::detail::{AttribType, AttribValue, Class, Detail};
use crate::spatial::PointGrid;
use glam::Vec3;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Scope, AST, FLOAT, INT};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Per-element operation budget. A real script is tens to hundreds of
/// operations; this is a loop that never ends, caught in milliseconds.
const OPS_PER_ELEMENT: u64 = 2_000_000;
/// Wall-clock budget for the whole run, so a script that is merely slow over
/// a large input is an error rather than a frozen UI.
const RUN_BUDGET: Duration = Duration::from_secs(8);
/// Compiled scripts kept by source. Small: a project has a handful of
/// wrangles, and each edit of one is a new key.
const AST_CACHE_CAP: usize = 64;

/// A channel value resolved before the run: what `ch` / `chs` / `chv` read.
#[derive(Clone, Debug, PartialEq)]
pub struct Chan {
    pub num: f64,
    pub text: String,
}

impl Chan {
    pub fn new(num: f64, text: impl Into<String>) -> Self {
        Chan { num, text: text.into() }
    }

    /// `chv`: three `:`-separated components, or the number splatted.
    fn vec(&self) -> Vec3 {
        let parts: Vec<f32> = self.text.split(':').filter_map(|p| p.trim().parse::<f32>().ok()).collect();
        if parts.len() == 3 {
            Vec3::new(parts[0], parts[1], parts[2])
        } else {
            Vec3::splat(self.num as f32)
        }
    }
}

/// The Class parameter: which element the script runs once per.
pub fn parse_class(s: &str) -> Class {
    let t = s.trim();
    if t.eq_ignore_ascii_case("primitives") || t.eq_ignore_ascii_case("prims") || t.eq_ignore_ascii_case("primitive") {
        Class::Prim
    } else if t.eq_ignore_ascii_case("detail") {
        Class::Detail
    } else {
        Class::Point
    }
}

/// `@name` → `__at["name"]`, outside strings and comments.
///
/// Rhai has no `@` token, so this is the whole of the sugar: an identifier
/// after `@` becomes an index on the element marker, and everything after it
/// — `.x`, `+=`, `[0]` — is Rhai's own syntax on the value that comes back.
/// String literals (double-quoted, backtick, and char literals) and both
/// comment forms are copied through untouched, so `"@"` in a message stays a
/// character.
pub fn desugar(code: &str) -> String {
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len() + 32);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Comments.
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            let end = code[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push_str(&code[i..end]);
            i = end;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let end = code[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            out.push_str(&code[i..end]);
            i = end;
            continue;
        }
        // String and char literals: copy to the matching close, honouring
        // backslash escapes.
        if c == b'"' || c == b'`' || c == b'\'' {
            let quote = c;
            let mut j = i + 1;
            while j < b.len() {
                if b[j] == b'\\' && quote != b'`' {
                    j += 2;
                    continue;
                }
                if b[j] == quote {
                    j += 1;
                    break;
                }
                j += 1;
            }
            let j = j.min(b.len());
            out.push_str(&code[i..j]);
            i = j;
            continue;
        }
        if c == b'@' && i + 1 < b.len() && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            out.push_str("__at[\"");
            out.push_str(&code[i + 1..j]);
            out.push_str("\"]");
            i = j;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// The channel paths a script names as string literals — `ch("../Radius")`,
/// `chs`, `chv`, `chi`, `chf`, `chb` — so the caller can resolve them before
/// the run. A path built at runtime is not found here and errors when read.
pub fn channel_refs(code: &str) -> Vec<String> {
    let src = strip_comments(code);
    let b = src.as_bytes();
    let mut out: Vec<String> = Vec::new();
    for call in ["ch(", "chs(", "chv(", "chi(", "chf(", "chb("] {
        let mut from = 0;
        while let Some(pos) = src[from..].find(call) {
            let at = from + pos;
            from = at + call.len();
            // A whole identifier: `search(` contains `ch(` and must not count.
            if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
                continue;
            }
            let rest = src[from..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
            let inner = &rest[1..];
            let Some(end) = inner.find(quote) else { continue };
            let path = inner[..end].to_string();
            if !path.is_empty() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn strip_comments(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("//") {
            let end = stripped.find('\n').unwrap_or(stripped.len());
            rest = &stripped[end..];
        } else if let Some(stripped) = rest.strip_prefix("/*") {
            let end = stripped.find("*/").map_or(stripped.len(), |n| n + 2);
            rest = &stripped[end..];
        } else {
            let mut chars = rest.chars();
            out.push(chars.next().unwrap());
            rest = chars.as_str();
        }
    }
    out
}

// ------------------------------------------------------------ the binding

/// The marker the desugared `@` indexes: `__at["P"]`. Carries nothing; the
/// indexers reach the shared context through their captured handle.
#[derive(Clone, Copy)]
struct El;

/// What the script runs against: the geometry, which element it is on, and
/// the edits it has asked for that apply after the run.
struct Ctx {
    d: Detail,
    class: Class,
    i: usize,
    frame: i32,
    normals: Option<Vec<Vec3>>,
    grid: Option<(f32, PointGrid)>,
    adds: Vec<Vec3>,
    prim_adds: Vec<Vec<u32>>,
    removes: Vec<usize>,
    chans: HashMap<String, Result<Chan, String>>,
}

type Shared = Rc<RefCell<Ctx>>;
type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

fn rt<T>(msg: impl Into<String>) -> RhaiResult<T> {
    Err(msg.into().into())
}

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Point => "point",
        Class::Vertex => "vertex",
        Class::Prim => "primitive",
        Class::Detail => "detail",
    }
}

/// A number out of anything numeric the script can hold.
fn num(v: &Dynamic) -> Result<f64, String> {
    if v.is_float() {
        Ok(v.as_float().unwrap_or(0.0))
    } else if v.is_int() {
        Ok(v.as_int().unwrap_or(0) as f64)
    } else if v.is_bool() {
        Ok(if v.as_bool().unwrap_or(false) { 1.0 } else { 0.0 })
    } else {
        Err(format!("expected a number, got {}", v.type_name()))
    }
}

fn to_vec3(v: &Dynamic) -> Result<Vec3, String> {
    if let Some(x) = v.clone().try_cast::<Vec3>() {
        return Ok(x);
    }
    if v.is_array() {
        let a = v.clone().into_array().unwrap_or_default();
        if a.len() >= 3 {
            return Ok(Vec3::new(num(&a[0])? as f32, num(&a[1])? as f32, num(&a[2])? as f32));
        }
        if a.len() == 2 {
            return Ok(Vec3::new(num(&a[0])? as f32, num(&a[1])? as f32, 0.0));
        }
        return Err(format!("expected a vec3, got an array of {}", a.len()));
    }
    Ok(Vec3::splat(num(v)? as f32))
}

fn to_dyn(v: AttribValue) -> Dynamic {
    match v {
        AttribValue::Float(f) => Dynamic::from_float(f as FLOAT),
        AttribValue::Int(i) => Dynamic::from_int(i as INT),
        AttribValue::Float3(a) => Dynamic::from(Vec3::from(a)),
        AttribValue::Float2(a) => Dynamic::from(Vec3::new(a[0], a[1], 0.0)),
        AttribValue::Float4(a) => Dynamic::from_array(a.iter().map(|&f| Dynamic::from_float(f as FLOAT)).collect()),
    }
}

/// A script value converted to an attribute's type — the type of the
/// attribute being written, so a whole number into a float attribute is a
/// float and a float into an int attribute is truncated, as the kernel
/// vocabulary does.
fn to_attr(v: &Dynamic, ty: AttribType) -> Result<AttribValue, String> {
    Ok(match ty {
        AttribType::Float => AttribValue::Float(to_scalar(v)? as f32),
        AttribType::Int => AttribValue::Int(to_scalar(v)?.trunc() as i32),
        AttribType::Float3 => AttribValue::Float3(to_vec3(v)?.to_array()),
        AttribType::Float2 => {
            let a = to_vec3(v)?;
            AttribValue::Float2([a.x, a.y])
        }
        AttribType::Float4 => {
            if v.is_array() {
                let a = v.clone().into_array().unwrap_or_default();
                if a.len() != 4 {
                    return Err(format!("expected four components, got {}", a.len()));
                }
                AttribValue::Float4([num(&a[0])? as f32, num(&a[1])? as f32, num(&a[2])? as f32, num(&a[3])? as f32])
            } else if let Some(x) = v.clone().try_cast::<Vec3>() {
                AttribValue::Float4([x.x, x.y, x.z, 1.0])
            } else {
                AttribValue::Float4([num(v)? as f32; 4])
            }
        }
    })
}

/// A scalar out of a number or a vector's first component.
fn to_scalar(v: &Dynamic) -> Result<f64, String> {
    if let Some(x) = v.clone().try_cast::<Vec3>() {
        return Ok(x.x as f64);
    }
    num(v)
}

/// The attribute type a fresh attribute takes from the first value written.
fn infer_type(v: &Dynamic) -> Result<AttribType, String> {
    if v.is_float() {
        Ok(AttribType::Float)
    } else if v.is_int() || v.is_bool() {
        Ok(AttribType::Int)
    } else if v.clone().try_cast::<Vec3>().is_some() {
        Ok(AttribType::Float3)
    } else if v.is_array() {
        match v.clone().into_array().unwrap_or_default().len() {
            2 => Ok(AttribType::Float2),
            3 => Ok(AttribType::Float3),
            4 => Ok(AttribType::Float4),
            n => Err(format!("an array of {n} is not an attribute type (2, 3 or 4 components)")),
        }
    } else {
        Err(format!("a {} cannot be stored as an attribute", v.type_name()))
    }
}

fn zero_of(ty: AttribType) -> AttribValue {
    match ty {
        AttribType::Float => AttribValue::Float(0.0),
        AttribType::Int => AttribValue::Int(0),
        AttribType::Float2 => AttribValue::Float2([0.0; 2]),
        AttribType::Float3 => AttribValue::Float3([0.0; 3]),
        AttribType::Float4 => AttribValue::Float4([0.0; 4]),
    }
}

impl Ctx {
    fn check(&self, class: Class, i: usize) -> Result<(), String> {
        let n = match class {
            Class::Point => self.d.num_points(),
            Class::Prim => self.d.num_prims(),
            Class::Vertex => self.d.num_verts(),
            Class::Detail => 1,
        };
        if i >= n {
            return Err(format!("{} {i} is out of range ({n} {}s)", class_name(class), class_name(class)));
        }
        Ok(())
    }

    /// One element's attribute, or one of the intrinsics that read like one.
    fn read(&mut self, class: Class, i: usize, name: &str) -> Result<Dynamic, String> {
        match name {
            "ptnum" | "primnum" | "elemnum" => return Ok(Dynamic::from_int(i as INT)),
            "numpt" => return Ok(Dynamic::from_int(self.d.num_points() as INT)),
            "numprim" => return Ok(Dynamic::from_int(self.d.num_prims() as INT)),
            "Frame" => return Ok(Dynamic::from_int(self.frame as INT)),
            _ => {}
        }
        self.check(class, i)?;
        match (class, name) {
            (Class::Point, "P") => Ok(Dynamic::from(self.d.pos(i))),
            (Class::Prim, "P") => {
                let pts = self.d.prim_points(i);
                let sum: Vec3 = pts.iter().map(|&p| self.d.pos(p as usize)).sum();
                Ok(Dynamic::from(if pts.is_empty() { Vec3::ZERO } else { sum / pts.len() as f32 }))
            }
            (Class::Point, "Cd") => Ok(Dynamic::from(Vec3::from(self.d.color(i)))),
            (Class::Point, "id") => Ok(Dynamic::from_int(self.d.id(i).unwrap_or(0) as INT)),
            (Class::Point, "N") if !self.d.points().has("N") => {
                if self.normals.is_none() {
                    self.normals = Some(crate::geometry::point_normals(&self.d));
                }
                Ok(Dynamic::from(self.normals.as_ref().unwrap()[i]))
            }
            _ => match self.d.store(class).value(name, i) {
                Some(v) => Ok(to_dyn(v)),
                None => Ok(Dynamic::from_float(0.0)),
            },
        }
    }

    fn write(&mut self, class: Class, i: usize, name: &str, v: &Dynamic) -> Result<(), String> {
        match name {
            "ptnum" | "primnum" | "elemnum" | "numpt" | "numprim" | "Frame" | "id" => {
                return Err(format!("@{name} is read-only"));
            }
            _ => {}
        }
        self.check(class, i)?;
        match (class, name) {
            (Class::Point, "P") => {
                self.d.set_pos(i, to_vec3(v)?);
                return Ok(());
            }
            (Class::Point, "Cd") => {
                self.d.set_color(i, to_vec3(v)?.to_array());
                return Ok(());
            }
            (_, "P") => return Err(format!("@P is read-only on a {}", class_name(class))),
            _ => {}
        }
        let store = self.d.store_mut(class);
        let ty = match store.get(name) {
            Some(data) => data.ty(),
            None => {
                let ty = infer_type(v)?;
                store.create(name, zero_of(ty));
                ty
            }
        };
        store.set_value(name, i, to_attr(v, ty)?)
    }
}

fn wrap<T>(r: Result<T, String>) -> RhaiResult<T> {
    r.map_err(|e| e.into())
}

fn index_arg(v: &Dynamic) -> Result<usize, String> {
    let n = num(v)?;
    if n < 0.0 {
        return Err(format!("index {n} is negative"));
    }
    Ok(n as usize)
}

/// A deterministic unit float from a seed: splitmix64 over the seed's bits,
/// so `rand(@ptnum)` and `rand(@P.x * 7.3)` both give a stable draw.
fn rand_unit(seed: &Dynamic) -> f64 {
    let bits: u64 = if seed.is_float() {
        seed.as_float().unwrap_or(0.0).to_bits()
    } else {
        seed.as_int().unwrap_or(0) as u64
    };
    let mut z = bits.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn build_engine(ctx: &Shared) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(OPS_PER_ELEMENT);

    // Numbers mix. Rhai keeps ints and floats apart by default, and a script
    // that writes `@P.y * 2` should not have to know which it wrote.
    engine
        .register_fn("+", |a: INT, b: FLOAT| a as FLOAT + b)
        .register_fn("+", |a: FLOAT, b: INT| a + b as FLOAT)
        .register_fn("-", |a: INT, b: FLOAT| a as FLOAT - b)
        .register_fn("-", |a: FLOAT, b: INT| a - b as FLOAT)
        .register_fn("*", |a: INT, b: FLOAT| a as FLOAT * b)
        .register_fn("*", |a: FLOAT, b: INT| a * b as FLOAT)
        .register_fn("/", |a: INT, b: FLOAT| a as FLOAT / b)
        .register_fn("/", |a: FLOAT, b: INT| a / b as FLOAT)
        .register_fn("%", |a: INT, b: FLOAT| (a as FLOAT) % b)
        .register_fn("%", |a: FLOAT, b: INT| a % b as FLOAT)
        .register_fn("**", |a: INT, b: FLOAT| (a as FLOAT).powf(b))
        .register_fn("**", |a: FLOAT, b: INT| a.powf(b as FLOAT))
        .register_fn("<", |a: INT, b: FLOAT| (a as FLOAT) < b)
        .register_fn("<", |a: FLOAT, b: INT| a < b as FLOAT)
        .register_fn(">", |a: INT, b: FLOAT| (a as FLOAT) > b)
        .register_fn(">", |a: FLOAT, b: INT| a > b as FLOAT)
        .register_fn("<=", |a: INT, b: FLOAT| (a as FLOAT) <= b)
        .register_fn("<=", |a: FLOAT, b: INT| a <= b as FLOAT)
        .register_fn(">=", |a: INT, b: FLOAT| (a as FLOAT) >= b)
        .register_fn(">=", |a: FLOAT, b: INT| a >= b as FLOAT)
        .register_fn("==", |a: INT, b: FLOAT| (a as FLOAT) == b)
        .register_fn("==", |a: FLOAT, b: INT| a == b as FLOAT)
        .register_fn("!=", |a: INT, b: FLOAT| (a as FLOAT) != b)
        .register_fn("!=", |a: FLOAT, b: INT| a != b as FLOAT);

    // vec3: glam's, with components, arithmetic and the usual functions.
    engine
        .register_type_with_name::<Vec3>("vec3")
        .register_get_set("x", |v: &mut Vec3| v.x as FLOAT, |v: &mut Vec3, x: FLOAT| v.x = x as f32)
        .register_get_set("y", |v: &mut Vec3| v.y as FLOAT, |v: &mut Vec3, y: FLOAT| v.y = y as f32)
        .register_get_set("z", |v: &mut Vec3| v.z as FLOAT, |v: &mut Vec3, z: FLOAT| v.z = z as f32)
        .register_set("x", |v: &mut Vec3, x: INT| v.x = x as f32)
        .register_set("y", |v: &mut Vec3, y: INT| v.y = y as f32)
        .register_set("z", |v: &mut Vec3, z: INT| v.z = z as f32)
        .register_indexer_get(|v: &mut Vec3, i: INT| -> RhaiResult<FLOAT> {
            match i {
                0 => Ok(v.x as FLOAT),
                1 => Ok(v.y as FLOAT),
                2 => Ok(v.z as FLOAT),
                _ => rt(format!("vec3 index {i} out of range")),
            }
        })
        .register_indexer_set(|v: &mut Vec3, i: INT, x: Dynamic| -> RhaiResult<()> {
            let x = wrap(num(&x))? as f32;
            match i {
                0 => v.x = x,
                1 => v.y = x,
                2 => v.z = x,
                _ => return rt(format!("vec3 index {i} out of range")),
            }
            Ok(())
        })
        .register_fn("vec3", |x: Dynamic, y: Dynamic, z: Dynamic| -> RhaiResult<Vec3> {
            Ok(Vec3::new(wrap(num(&x))? as f32, wrap(num(&y))? as f32, wrap(num(&z))? as f32))
        })
        .register_fn("vec3", |x: Dynamic| -> RhaiResult<Vec3> { wrap(to_vec3(&x)) })
        .register_fn("+", |a: Vec3, b: Vec3| a + b)
        .register_fn("-", |a: Vec3, b: Vec3| a - b)
        .register_fn("-", |a: Vec3| -a)
        .register_fn("*", |a: Vec3, b: Vec3| a * b)
        .register_fn("*", |a: Vec3, b: FLOAT| a * b as f32)
        .register_fn("*", |a: FLOAT, b: Vec3| b * a as f32)
        .register_fn("*", |a: Vec3, b: INT| a * b as f32)
        .register_fn("*", |a: INT, b: Vec3| b * a as f32)
        .register_fn("/", |a: Vec3, b: Vec3| a / b)
        .register_fn("/", |a: Vec3, b: FLOAT| a / b as f32)
        .register_fn("/", |a: Vec3, b: INT| a / b as f32)
        .register_fn("==", |a: Vec3, b: Vec3| a == b)
        .register_fn("!=", |a: Vec3, b: Vec3| a != b)
        .register_fn("to_string", |v: Vec3| format!("{}:{}:{}", v.x, v.y, v.z))
        .register_fn("to_debug", |v: Vec3| format!("vec3({}, {}, {})", v.x, v.y, v.z))
        .register_fn("dot", |a: Vec3, b: Vec3| a.dot(b) as FLOAT)
        .register_fn("cross", |a: Vec3, b: Vec3| a.cross(b))
        .register_fn("length", |a: Vec3| a.length() as FLOAT)
        .register_fn("length2", |a: Vec3| a.length_squared() as FLOAT)
        .register_fn("normalize", |a: Vec3| a.normalize_or_zero())
        .register_fn("distance", |a: Vec3, b: Vec3| a.distance(b) as FLOAT)
        .register_fn("abs", |a: Vec3| a.abs())
        .register_fn("min", |a: Vec3, b: Vec3| a.min(b))
        .register_fn("max", |a: Vec3, b: Vec3| a.max(b))
        .register_fn("lerp", |a: Vec3, b: Vec3, t: Dynamic| -> RhaiResult<Vec3> { Ok(a.lerp(b, wrap(num(&t))? as f32)) })
        .register_fn("lerp", |a: FLOAT, b: FLOAT, t: FLOAT| a + (b - a) * t)
        .register_fn("clamp", |v: Vec3, lo: Vec3, hi: Vec3| v.clamp(lo, hi))
        .register_fn("clamp", |v: Dynamic, lo: Dynamic, hi: Dynamic| -> RhaiResult<FLOAT> {
            let (v, lo, hi) = (wrap(num(&v))?, wrap(num(&lo))?, wrap(num(&hi))?);
            Ok(v.max(lo).min(hi))
        })
        .register_fn("fit", |v: Dynamic, a: Dynamic, b: Dynamic, c: Dynamic, d: Dynamic| -> RhaiResult<FLOAT> {
            let (v, a, b, c, d) = (wrap(num(&v))?, wrap(num(&a))?, wrap(num(&b))?, wrap(num(&c))?, wrap(num(&d))?);
            let t = if (b - a).abs() < 1e-12 { 0.0 } else { ((v - a) / (b - a)).clamp(0.0, 1.0) };
            Ok(c + (d - c) * t)
        })
        .register_fn("rand", |seed: Dynamic| rand_unit(&seed));

    // The element: `@name` desugars to `__at["name"]`.
    engine.register_type_with_name::<El>("element");
    let c = ctx.clone();
    engine.register_indexer_get(move |_: &mut El, name: ImmutableString| -> RhaiResult<Dynamic> {
        let mut c = c.borrow_mut();
        let (class, i) = (c.class, c.i);
        wrap(c.read(class, i, &name))
    });
    let c = ctx.clone();
    engine.register_indexer_set(move |_: &mut El, name: ImmutableString, v: Dynamic| -> RhaiResult<()> {
        let mut c = c.borrow_mut();
        let (class, i) = (c.class, c.i);
        wrap(c.write(class, i, &name, &v))
    });

    // Other elements, by index.
    let c = ctx.clone();
    engine.register_fn("point", move |name: ImmutableString, i: Dynamic| -> RhaiResult<Dynamic> {
        let i = wrap(index_arg(&i))?;
        wrap(c.borrow_mut().read(Class::Point, i, &name))
    });
    let c = ctx.clone();
    engine.register_fn("setpoint", move |name: ImmutableString, i: Dynamic, v: Dynamic| -> RhaiResult<()> {
        let i = wrap(index_arg(&i))?;
        wrap(c.borrow_mut().write(Class::Point, i, &name, &v))
    });
    let c = ctx.clone();
    engine.register_fn("prim", move |name: ImmutableString, i: Dynamic| -> RhaiResult<Dynamic> {
        let i = wrap(index_arg(&i))?;
        wrap(c.borrow_mut().read(Class::Prim, i, &name))
    });
    let c = ctx.clone();
    engine.register_fn("setprim", move |name: ImmutableString, i: Dynamic, v: Dynamic| -> RhaiResult<()> {
        let i = wrap(index_arg(&i))?;
        wrap(c.borrow_mut().write(Class::Prim, i, &name, &v))
    });
    let c = ctx.clone();
    engine.register_fn("detail", move |name: ImmutableString| -> RhaiResult<Dynamic> {
        wrap(c.borrow_mut().read(Class::Detail, 0, &name))
    });
    let c = ctx.clone();
    engine.register_fn("setdetail", move |name: ImmutableString, v: Dynamic| -> RhaiResult<()> {
        wrap(c.borrow_mut().write(Class::Detail, 0, &name, &v))
    });
    let c = ctx.clone();
    engine.register_fn("npoints", move || c.borrow().d.num_points() as INT);
    let c = ctx.clone();
    engine.register_fn("nprims", move || c.borrow().d.num_prims() as INT);

    // Topology.
    let c = ctx.clone();
    engine.register_fn("neighbours", move |i: Dynamic| -> RhaiResult<Array> {
        let i = wrap(index_arg(&i))?;
        let c = c.borrow();
        wrap(c.check(Class::Point, i))?;
        Ok(c.d.point_neighbours(i).iter().map(|&n| Dynamic::from_int(n as INT)).collect())
    });
    let c = ctx.clone();
    engine.register_fn("neighbors", move |i: Dynamic| -> RhaiResult<Array> {
        let i = wrap(index_arg(&i))?;
        let c = c.borrow();
        wrap(c.check(Class::Point, i))?;
        Ok(c.d.point_neighbours(i).iter().map(|&n| Dynamic::from_int(n as INT)).collect())
    });
    let c = ctx.clone();
    engine.register_fn("prims", move |i: Dynamic| -> RhaiResult<Array> {
        let i = wrap(index_arg(&i))?;
        let c = c.borrow();
        wrap(c.check(Class::Point, i))?;
        Ok(c.d.point_prims(i).iter().map(|&n| Dynamic::from_int(n as INT)).collect())
    });
    let c = ctx.clone();
    engine.register_fn("points", move |i: Dynamic| -> RhaiResult<Array> {
        let i = wrap(index_arg(&i))?;
        let c = c.borrow();
        wrap(c.check(Class::Prim, i))?;
        Ok(c.d.prim_points(i).iter().map(|&n| Dynamic::from_int(n as INT)).collect())
    });
    let c = ctx.clone();
    engine.register_fn("nearest", move |pos: Dynamic, radius: Dynamic| -> RhaiResult<Array> {
        let pos = wrap(to_vec3(&pos))?;
        let radius = wrap(num(&radius))? as f32;
        if radius <= 0.0 {
            return Ok(Array::new());
        }
        let mut c = c.borrow_mut();
        // The grid is built from the positions as they stand at the first
        // call and keyed by radius; a script that moves points and asks
        // again reads the earlier layout, which is what a per-element pass
        // should see anyway.
        if c.grid.as_ref().map_or(true, |(r, _)| (*r - radius).abs() > 1e-6) {
            let pts: Vec<Vec3> = (0..c.d.num_points()).map(|p| c.d.pos(p)).collect();
            c.grid = Some((radius, PointGrid::build(&pts, radius)));
        }
        let mut out = Vec::new();
        c.grid.as_ref().unwrap().1.within(pos, radius, &mut out);
        out.sort_by(|&a, &b| {
            let da = c.d.pos(a as usize).distance_squared(pos);
            let db = c.d.pos(b as usize).distance_squared(pos);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out.into_iter().map(|n| Dynamic::from_int(n as INT)).collect())
    });

    // Groups.
    let c = ctx.clone();
    engine.register_fn("ingroup", move |name: ImmutableString| -> bool {
        let c = c.borrow();
        c.d.store(c.class).in_group(&name, c.i)
    });
    let c = ctx.clone();
    engine.register_fn("ingroup", move |name: ImmutableString, i: Dynamic| -> RhaiResult<bool> {
        let i = wrap(index_arg(&i))?;
        let c = c.borrow();
        Ok(c.d.store(c.class).in_group(&name, i))
    });
    let c = ctx.clone();
    engine.register_fn("setgroup", move |name: ImmutableString, on: Dynamic| -> RhaiResult<()> {
        let on = wrap(num(&on))? != 0.0;
        let mut c = c.borrow_mut();
        let (class, i) = (c.class, c.i);
        let store = c.d.store_mut(class);
        if on {
            if !store.has_group(&name) {
                store.create_group(&name);
            }
            store.add_to_group(&name, i);
        } else if store.has_group(&name) {
            // No remove-one on the store: rebuild the membership without i.
            let members: Vec<u32> = store.group_members(&name).into_iter().filter(|&m| m as usize != i).collect();
            store.remove_group(&name);
            store.create_group(&name);
            for m in members {
                store.add_to_group(&name, m as usize);
            }
        }
        Ok(())
    });

    // Deferred structural edits.
    let c = ctx.clone();
    engine.register_fn("addpoint", move |pos: Dynamic| -> RhaiResult<INT> {
        let pos = wrap(to_vec3(&pos))?;
        let mut c = c.borrow_mut();
        let idx = c.d.num_points() + c.adds.len();
        c.adds.push(pos);
        Ok(idx as INT)
    });
    let c = ctx.clone();
    engine.register_fn("addprim", move |pts: Array| -> RhaiResult<INT> {
        let mut c = c.borrow_mut();
        let total = c.d.num_points() + c.adds.len();
        let mut idx = Vec::with_capacity(pts.len());
        for p in &pts {
            let p = wrap(index_arg(p))?;
            if p >= total {
                return rt(format!("addprim: point {p} does not exist ({total} points)"));
            }
            idx.push(p as u32);
        }
        if idx.len() < 2 {
            return rt("addprim: a primitive needs at least two points");
        }
        let n = c.d.num_prims() + c.prim_adds.len();
        c.prim_adds.push(idx);
        Ok(n as INT)
    });
    let c = ctx.clone();
    engine.register_fn("removepoint", move |i: Dynamic| -> RhaiResult<()> {
        let i = wrap(index_arg(&i))?;
        let mut c = c.borrow_mut();
        wrap(c.check(Class::Point, i))?;
        c.removes.push(i);
        Ok(())
    });

    // Channels, resolved before the run.
    fn chan(c: &Shared, path: &str) -> RhaiResult<Chan> {
        match c.borrow().chans.get(path) {
            Some(Ok(ch)) => Ok(ch.clone()),
            Some(Err(e)) => rt(e.clone()),
            None => rt(format!("ch(\"{path}\"): a channel path must be a string literal, so it can be resolved before the script runs")),
        }
    }
    let c = ctx.clone();
    engine.register_fn("ch", move |path: ImmutableString| -> RhaiResult<FLOAT> { Ok(chan(&c, &path)?.num) });
    let c = ctx.clone();
    engine.register_fn("chf", move |path: ImmutableString| -> RhaiResult<FLOAT> { Ok(chan(&c, &path)?.num) });
    let c = ctx.clone();
    engine.register_fn("chi", move |path: ImmutableString| -> RhaiResult<INT> { Ok(chan(&c, &path)?.num.trunc() as INT) });
    let c = ctx.clone();
    engine.register_fn("chb", move |path: ImmutableString| -> RhaiResult<bool> { Ok(chan(&c, &path)?.num != 0.0) });
    let c = ctx.clone();
    engine.register_fn("chs", move |path: ImmutableString| -> RhaiResult<ImmutableString> { Ok(chan(&c, &path)?.text.into()) });
    let c = ctx.clone();
    engine.register_fn("chv", move |path: ImmutableString| -> RhaiResult<Vec3> { Ok(chan(&c, &path)?.vec()) });

    engine
}

thread_local! {
    static AST_CACHE: RefCell<HashMap<String, Rc<AST>>> = RefCell::new(HashMap::new());
}

fn compile(engine: &Engine, src: &str) -> Result<Rc<AST>, String> {
    if let Some(ast) = AST_CACHE.with(|c| c.borrow().get(src).cloned()) {
        return Ok(ast);
    }
    let ast = Rc::new(engine.compile(src).map_err(|e| format!("syntax: {e}"))?);
    AST_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= AST_CACHE_CAP {
            c.clear();
        }
        c.insert(src.to_string(), ast.clone());
    });
    Ok(ast)
}

/// Run `code` once per element of `class` in `input` (over `group`, if
/// named), with the channel values the caller resolved. On any error — a
/// syntax error, a runtime error on some element, the budget — the whole run
/// fails and the caller keeps its input: a half-wrangled geometry is not a
/// result.
pub fn run_wrangle(
    input: Detail,
    code: &str,
    class: Class,
    group: &str,
    frame: i32,
    chans: HashMap<String, Result<Chan, String>>,
) -> Result<Detail, String> {
    let class = match class {
        Class::Vertex => Class::Point,
        c => c,
    };
    let ctx: Shared = Rc::new(RefCell::new(Ctx {
        d: input,
        class,
        i: 0,
        frame,
        normals: None,
        grid: None,
        adds: Vec::new(),
        prim_adds: Vec::new(),
        removes: Vec::new(),
        chans,
    }));
    let mut engine = build_engine(&ctx);

    // The wall-clock budget, checked every so often rather than per operation.
    let started = Instant::now();
    let ticks = Cell::new(0u32);
    engine.on_progress(move |_| {
        ticks.set(ticks.get().wrapping_add(1));
        if ticks.get() % 4096 == 0 && started.elapsed() > RUN_BUDGET {
            Some(Dynamic::from(format!("the script ran for more than {} s and was stopped", RUN_BUDGET.as_secs())))
        } else {
            None
        }
    });

    let src = desugar(code);
    let ast = compile(&engine, &src)?;

    let group = group.trim();
    let elements: Vec<usize> = {
        let c = ctx.borrow();
        let n = match class {
            Class::Point => c.d.num_points(),
            Class::Prim => c.d.num_prims(),
            _ => 1,
        };
        (0..n).filter(|&i| class == Class::Detail || group.is_empty() || c.d.store(class).in_group(group, i)).collect()
    };

    let mut scope = Scope::new();
    // A variable, not a constant: Rhai refuses to assign through an indexer
    // on a constant, and `@P = ...` is exactly that.
    scope.push("__at", El);
    let base = scope.len();
    for i in elements {
        ctx.borrow_mut().i = i;
        scope.rewind(base);
        if let Err(e) = engine.run_ast_with_scope(&mut scope, &ast) {
            let e = e.to_string();
            let e = e.strip_prefix("Runtime error: ").unwrap_or(&e).to_string();
            return Err(match class {
                Class::Detail => e,
                _ => format!("{} {i}: {e}", class_name(class)),
            });
        }
    }

    drop(scope);
    drop(engine);
    let mut c = ctx.borrow_mut();
    let mut d = std::mem::replace(&mut c.d, Detail::new());
    let adds = std::mem::take(&mut c.adds);
    let prim_adds = std::mem::take(&mut c.prim_adds);
    let removes = std::mem::take(&mut c.removes);
    drop(c);

    for p in adds {
        d.add_point(p);
    }
    for pts in prim_adds {
        d.add_prim(&pts);
    }
    if !removes.is_empty() {
        let mut keep = vec![true; d.num_points()];
        for r in removes {
            if r < keep.len() {
                keep[r] = false;
            }
        }
        d.keep_points(&keep);
    }
    Ok(d)
}
