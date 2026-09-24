//! The geometry container: points, vertices, primitives and detail.
//!
//! This is the Phase 0 replacement for `geometry::Geometry`, the triangle soup
//! (`Vec<GVertex>`, attributes stored per triangle corner). See
//! `shapeshifter.md` for why: every attribute operator worth having is a
//! statement about a point *and its neighbours*, and a soup has no points, no
//! edges, and no identity that survives a frame.
//!
//! Four element classes, exactly Houdini's:
//!
//! - **Points** carry position and the attributes that describe a place on the
//!   surface. A point is shared by every primitive that uses it — moving one
//!   moves them all, which is the whole difference from a soup.
//! - **Vertices** are a primitive's references to points, in winding order. One
//!   per corner. They exist so a point can carry different per-corner data (a
//!   UV seam, a hard normal) without splitting the point itself.
//! - **Primitives** are runs of vertices. Polygons of any size, not just
//!   triangles; triangulation is a render concern (see [`Detail::triangulate`]).
//! - **Detail** is the single-element class: one row holding whole-geometry
//!   values, which is where `Analysis` writes a range rather than inventing a
//!   dictionary type.
//!
//! Three properties the soup could not have, all load-bearing for later phases:
//!
//! **Columnar attributes.** One array per named attribute, not a `HashMap` per
//! element. This is the layout a GPU buffer already wants, so the Phase 1 kernel
//! ABI binds a slice instead of marshalling a million little maps.
//!
//! **Stable point ids.** [`PointId`] is allocated once per point and preserved by
//! every operator that does not create points. A solver needs to know that the
//! point it is looking at is the one it wrote to last step; a positional weld
//! cannot answer that, because points move.
//!
//! **Real groups.** A named membership set per class, not the `group:<name>`
//! key convention the soup used in its attribute map.
//!
//! Topology (point→prim, point→point, the edge list) is *derived*, built lazily
//! on first ask and dropped on any structural edit — so a chain of ten attribute
//! nodes builds it once rather than welding from scratch in every node, which is
//! what `resolve_relax_geometry_with_errors` has to do today.

use glam::Vec3;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A point's identity, stable across the operators that preserve points and
/// across simulation steps. Allocated by [`Detail::add_point`]; never reused
/// within one `Detail`.
pub type PointId = u64;

/// The conventional color attribute. Read by [`Detail::triangulate`] when
/// present; geometry without it renders at [`DEFAULT_COLOR`].
pub const CD: &str = "Cd";

/// Point attributes under this prefix are MARKER REQUESTS, not data: the
/// Visualize node copies a vector attribute into `vis_<name>`, already scaled,
/// and the viewport draws a segment from each point along it.
///
/// A naming convention rather than a side-channel on the container, so the
/// request travels with the geometry through every operator that already knows
/// how to carry an attribute, and shows up in the spreadsheet where you can
/// see what is being drawn and why.
pub const VIS_PREFIX: &str = "vis_";

/// What a point renders as when it carries no `Cd`.
pub const DEFAULT_COLOR: [f32; 3] = [0.8, 0.8, 0.8];

/// Which element class an attribute or group belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class {
    Point,
    Vertex,
    Prim,
    Detail,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Point => "point",
            Class::Vertex => "vertex",
            Class::Prim => "prim",
            Class::Detail => "detail",
        }
    }
}

/// An attribute's element type. Integers are here because the Developer set
/// needs counters and ages that stay whole — `Vitality` writes `_age0..2`, and
/// rounding a float age is how off-by-one frames happen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttribType {
    Float,
    Float2,
    Float3,
    Float4,
    Int,
}

impl AttribType {
    /// Component count. `Int` is one component, like `Float`.
    pub fn components(self) -> usize {
        match self {
            AttribType::Float | AttribType::Int => 1,
            AttribType::Float2 => 2,
            AttribType::Float3 => 3,
            AttribType::Float4 => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AttribType::Float => "float",
            AttribType::Float2 => "float2",
            AttribType::Float3 => "float3",
            AttribType::Float4 => "float4",
            AttribType::Int => "int",
        }
    }
}

/// One element's value, for the get/set paths that do not care about layout.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AttribValue {
    Float(f32),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
    Int(i32),
}

impl AttribValue {
    pub fn ty(self) -> AttribType {
        match self {
            AttribValue::Float(_) => AttribType::Float,
            AttribValue::Float2(_) => AttribType::Float2,
            AttribValue::Float3(_) => AttribType::Float3,
            AttribValue::Float4(_) => AttribType::Float4,
            AttribValue::Int(_) => AttribType::Int,
        }
    }

    /// The value as a float, for the readers that treat every scalar alike.
    /// Wider types yield their first component.
    pub fn as_f32(self) -> f32 {
        match self {
            AttribValue::Float(v) => v,
            AttribValue::Float2(v) => v[0],
            AttribValue::Float3(v) => v[0],
            AttribValue::Float4(v) => v[0],
            AttribValue::Int(v) => v as f32,
        }
    }

    /// The value as a vector, for the readers that treat every vector alike.
    /// Scalars broadcast across all three components, which is what a scalar
    /// used as a multiplier means.
    pub fn as_vec3(self) -> Vec3 {
        match self {
            AttribValue::Float(v) => Vec3::splat(v),
            AttribValue::Float2(v) => Vec3::new(v[0], v[1], 0.0),
            AttribValue::Float3(v) => Vec3::from(v),
            AttribValue::Float4(v) => Vec3::new(v[0], v[1], v[2]),
            AttribValue::Int(v) => Vec3::splat(v as f32),
        }
    }
}

/// Whether an attribute survives a simulation step.
///
/// The distinction `developer.md` draws between **live data**, which "runs
/// like a stream through the simulation", and **derivative data**, "calculated
/// anew every frame based on live data". Houdini has no such notion — there,
/// every attribute simply persists, and a chain that forgets to reset its
/// scratch values accumulates them silently until the sim goes wrong in a way
/// that looks like a physics bug.
///
/// Making it a property of the ATTRIBUTE rather than a list of names on the
/// solver means the node that creates a value declares its nature at the point
/// of creation, where the author knows the answer, instead of somewhere else
/// that has to be kept in step.
///
/// [`AttribKind::Live`] is the default, because it is the one whose failure
/// mode is visible: a value that should have been cleared and was not shows up
/// as a drift you can watch, where a value that should have persisted and was
/// cleared just quietly reads zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AttribKind {
    /// Carried across the step boundary, by point identity where the geometry
    /// was rebuilt underneath it.
    #[default]
    Live,
    /// Zeroed at the start of every step; the chain is expected to rebuild it.
    Derivative,
}

/// One attribute's storage: a single array covering every element of the
/// owning class, in element order.
#[derive(Clone, Debug, PartialEq)]
pub enum AttribData {
    Float(Vec<f32>),
    Float2(Vec<[f32; 2]>),
    Float3(Vec<[f32; 3]>),
    Float4(Vec<[f32; 4]>),
    Int(Vec<i32>),
}

impl AttribData {
    /// An array of `len` elements, every one the type's zero.
    pub fn zeroed(ty: AttribType, len: usize) -> Self {
        match ty {
            AttribType::Float => AttribData::Float(vec![0.0; len]),
            AttribType::Float2 => AttribData::Float2(vec![[0.0; 2]; len]),
            AttribType::Float3 => AttribData::Float3(vec![[0.0; 3]; len]),
            AttribType::Float4 => AttribData::Float4(vec![[0.0; 4]; len]),
            AttribType::Int => AttribData::Int(vec![0; len]),
        }
    }

    /// An array of `len` elements, every one `value`.
    pub fn filled(value: AttribValue, len: usize) -> Self {
        match value {
            AttribValue::Float(v) => AttribData::Float(vec![v; len]),
            AttribValue::Float2(v) => AttribData::Float2(vec![v; len]),
            AttribValue::Float3(v) => AttribData::Float3(vec![v; len]),
            AttribValue::Float4(v) => AttribData::Float4(vec![v; len]),
            AttribValue::Int(v) => AttribData::Int(vec![v; len]),
        }
    }

    pub fn ty(&self) -> AttribType {
        match self {
            AttribData::Float(_) => AttribType::Float,
            AttribData::Float2(_) => AttribType::Float2,
            AttribData::Float3(_) => AttribType::Float3,
            AttribData::Float4(_) => AttribType::Float4,
            AttribData::Int(_) => AttribType::Int,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            AttribData::Float(v) => v.len(),
            AttribData::Float2(v) => v.len(),
            AttribData::Float3(v) => v.len(),
            AttribData::Float4(v) => v.len(),
            AttribData::Int(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<AttribValue> {
        match self {
            AttribData::Float(v) => v.get(i).copied().map(AttribValue::Float),
            AttribData::Float2(v) => v.get(i).copied().map(AttribValue::Float2),
            AttribData::Float3(v) => v.get(i).copied().map(AttribValue::Float3),
            AttribData::Float4(v) => v.get(i).copied().map(AttribValue::Float4),
            AttribData::Int(v) => v.get(i).copied().map(AttribValue::Int),
        }
    }

    /// Write one element. The value's type must match the array's — a caller
    /// wanting to change an attribute's type replaces the whole array, so that
    /// a half-converted attribute is unrepresentable.
    pub fn set(&mut self, i: usize, value: AttribValue) -> Result<(), String> {
        macro_rules! put {
            ($arr:expr, $v:expr) => {{
                let len = $arr.len();
                let slot = $arr
                    .get_mut(i)
                    .ok_or_else(|| format!("element {} is out of range ({})", i, len))?;
                *slot = $v;
                Ok(())
            }};
        }
        match (self, value) {
            (AttribData::Float(a), AttribValue::Float(v)) => put!(a, v),
            (AttribData::Float2(a), AttribValue::Float2(v)) => put!(a, v),
            (AttribData::Float3(a), AttribValue::Float3(v)) => put!(a, v),
            (AttribData::Float4(a), AttribValue::Float4(v)) => put!(a, v),
            (AttribData::Int(a), AttribValue::Int(v)) => put!(a, v),
            (data, value) => Err(format!(
                "type mismatch: attribute is {}, value is {}",
                data.ty().name(),
                value.ty().name()
            )),
        }
    }

    /// Append one zero element, keeping the array in step with a class that
    /// just grew.
    pub fn push_zero(&mut self) {
        match self {
            AttribData::Float(v) => v.push(0.0),
            AttribData::Float2(v) => v.push([0.0; 2]),
            AttribData::Float3(v) => v.push([0.0; 3]),
            AttribData::Float4(v) => v.push([0.0; 4]),
            AttribData::Int(v) => v.push(0),
        }
    }

    /// Grow or shrink to `len`, zero-filling any new elements.
    pub fn resize(&mut self, len: usize) {
        match self {
            AttribData::Float(v) => v.resize(len, 0.0),
            AttribData::Float2(v) => v.resize(len, [0.0; 2]),
            AttribData::Float3(v) => v.resize(len, [0.0; 3]),
            AttribData::Float4(v) => v.resize(len, [0.0; 4]),
            AttribData::Int(v) => v.resize(len, 0),
        }
    }

    /// A new array holding this one's elements at `idx`, in that order.
    ///
    /// The single primitive every topology-changing operator needs: deleting
    /// elements, reordering them, and duplicating them are all a gather, so
    /// there is one place where "what happens to the attributes" is answered.
    /// Indices out of range contribute a zero rather than panicking — a caller
    /// building an index map should not be able to corrupt memory with an
    /// arithmetic slip.
    pub fn gather(&self, idx: &[u32]) -> AttribData {
        macro_rules! pick {
            ($arr:expr, $zero:expr, $wrap:path) => {{
                let mut out = Vec::with_capacity(idx.len());
                for &i in idx {
                    out.push($arr.get(i as usize).copied().unwrap_or($zero));
                }
                $wrap(out)
            }};
        }
        match self {
            AttribData::Float(a) => pick!(a, 0.0, AttribData::Float),
            AttribData::Float2(a) => pick!(a, [0.0; 2], AttribData::Float2),
            AttribData::Float3(a) => pick!(a, [0.0; 3], AttribData::Float3),
            AttribData::Float4(a) => pick!(a, [0.0; 4], AttribData::Float4),
            AttribData::Int(a) => pick!(a, 0, AttribData::Int),
        }
    }

    /// The raw floats behind the array, for a GPU upload or a bulk read.
    /// `Int` has no float view and yields `None`.
    pub fn as_f32_slice(&self) -> Option<&[f32]> {
        match self {
            AttribData::Float(v) => Some(v.as_slice()),
            AttribData::Float2(v) => Some(bytemuck::cast_slice(v.as_slice())),
            AttribData::Float3(v) => Some(bytemuck::cast_slice(v.as_slice())),
            AttribData::Float4(v) => Some(bytemuck::cast_slice(v.as_slice())),
            AttribData::Int(_) => None,
        }
    }
}

/// Every attribute and group belonging to one element class, plus the element
/// count they are all kept in step with.
#[derive(Clone, Debug, Default)]
pub struct AttribStore {
    len: usize,
    attribs: HashMap<String, AttribData>,
    /// Only the attributes that are NOT the default kind appear here, so an
    /// absent entry reads as [`AttribKind::Live`] and nothing has to remember
    /// to register an ordinary attribute.
    kinds: HashMap<String, AttribKind>,
    groups: HashMap<String, Vec<bool>>,
}

impl AttribStore {
    pub fn with_len(len: usize) -> Self {
        Self { len, attribs: HashMap::new(), kinds: HashMap::new(), groups: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Attribute names, sorted, so a spreadsheet's columns do not reshuffle
    /// between frames on `HashMap` iteration order.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.attribs.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Group names, sorted, for the same reason.
    pub fn group_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.groups.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    pub fn has(&self, name: &str) -> bool {
        self.attribs.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&AttribData> {
        self.attribs.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut AttribData> {
        self.attribs.get_mut(name)
    }

    /// Create (or replace) an attribute, every element set to `default`.
    ///
    /// The attribute is [`AttribKind::Live`]; use [`AttribStore::create_kind`]
    /// for one the solver should clear each step. Replacing an attribute
    /// replaces its kind too — a name reused for a different purpose is a
    /// different attribute.
    pub fn create(&mut self, name: &str, default: AttribValue) -> &mut AttribData {
        self.create_kind(name, default, AttribKind::Live)
    }

    /// Create (or replace) an attribute, declaring whether it survives a step.
    pub fn create_kind(
        &mut self,
        name: &str,
        default: AttribValue,
        kind: AttribKind,
    ) -> &mut AttribData {
        self.attribs
            .insert(name.to_string(), AttribData::filled(default, self.len));
        match kind {
            AttribKind::Live => self.kinds.remove(name),
            other => self.kinds.insert(name.to_string(), other),
        };
        self.attribs.get_mut(name).expect("just inserted")
    }

    /// Whether an attribute survives a simulation step. An attribute nobody
    /// declared is Live.
    pub fn kind(&self, name: &str) -> AttribKind {
        self.kinds.get(name).copied().unwrap_or_default()
    }

    /// Declare an existing attribute's kind without disturbing its values.
    pub fn set_kind(&mut self, name: &str, kind: AttribKind) {
        if !self.attribs.contains_key(name) {
            return;
        }
        match kind {
            AttribKind::Live => self.kinds.remove(name),
            other => self.kinds.insert(name.to_string(), other),
        };
    }

    /// The names of every attribute of one kind, sorted.
    pub fn names_of_kind(&self, kind: AttribKind) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .attribs
            .keys()
            .filter(|n| self.kind(n) == kind)
            .map(|s| s.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// Zero every Derivative attribute, keeping the columns themselves — the
    /// step that follows is expected to rebuild the values, and a reader
    /// between the two should find the attribute present and empty rather than
    /// missing.
    pub fn clear_derivatives(&mut self) {
        let names: Vec<String> = self.kinds
            .iter()
            .filter(|(_, &k)| k == AttribKind::Derivative)
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            if let Some(data) = self.attribs.get_mut(&name) {
                *data = AttribData::zeroed(data.ty(), self.len);
            }
        }
    }

    /// Create the attribute if it is absent, leaving an existing one — and its
    /// values — alone. The read path for an operator that wants to write into
    /// an attribute it does not own.
    pub fn get_or_create(&mut self, name: &str, default: AttribValue) -> &mut AttribData {
        if !self.attribs.contains_key(name) {
            self.create(name, default);
        }
        self.attribs.get_mut(name).expect("present or just created")
    }

    pub fn remove(&mut self, name: &str) -> Option<AttribData> {
        self.kinds.remove(name);
        self.attribs.remove(name)
    }

    /// Install a whole array as an attribute, which is how a generator that
    /// computed every value in one pass writes them — one move instead of an
    /// element-at-a-time walk.
    ///
    /// A length mismatch is refused rather than padded: an array that does not
    /// line up with its class is a caller bug, and silently zero-filling it
    /// would put the wrong value on every element after the first mistake.
    pub fn insert(&mut self, name: &str, data: AttribData) -> Result<(), String> {
        if data.len() != self.len {
            return Err(format!(
                "attribute {:?} has {} entries, the class has {}",
                name,
                data.len(),
                self.len
            ));
        }
        self.attribs.insert(name.to_string(), data);
        Ok(())
    }

    pub fn value(&self, name: &str, i: usize) -> Option<AttribValue> {
        self.attribs.get(name).and_then(|a| a.get(i))
    }

    pub fn set_value(&mut self, name: &str, i: usize, v: AttribValue) -> Result<(), String> {
        self.attribs
            .get_mut(name)
            .ok_or_else(|| format!("no attribute named {:?}", name))?
            .set(i, v)
    }

    /// Create an empty group, or empty an existing one.
    pub fn create_group(&mut self, name: &str) {
        self.groups.insert(name.to_string(), vec![false; self.len]);
    }

    pub fn has_group(&self, name: &str) -> bool {
        self.groups.contains_key(name)
    }

    pub fn remove_group(&mut self, name: &str) -> bool {
        self.groups.remove(name).is_some()
    }

    /// Put one element in a group, creating the group if needed. Out-of-range
    /// indices are ignored.
    pub fn add_to_group(&mut self, name: &str, i: usize) {
        let len = self.len;
        let members = self
            .groups
            .entry(name.to_string())
            .or_insert_with(|| vec![false; len]);
        if let Some(slot) = members.get_mut(i) {
            *slot = true;
        }
    }

    pub fn in_group(&self, name: &str, i: usize) -> bool {
        self.groups.get(name).and_then(|m| m.get(i)).copied().unwrap_or(false)
    }

    /// The members of a group, in element order. An absent group has no
    /// members — asking about a group nobody created is not an error, because
    /// a node's Group parameter is routinely left blank.
    pub fn group_members(&self, name: &str) -> Vec<u32> {
        match self.groups.get(name) {
            Some(m) => m
                .iter()
                .enumerate()
                .filter(|(_, &v)| v)
                .map(|(i, _)| i as u32)
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn group_len(&self, name: &str) -> usize {
        self.groups
            .get(name)
            .map(|m| m.iter().filter(|&&v| v).count())
            .unwrap_or(0)
    }

    /// Append one element's worth of room to every attribute and group.
    fn push_element(&mut self) {
        self.len += 1;
        for a in self.attribs.values_mut() {
            a.push_zero();
        }
        for g in self.groups.values_mut() {
            g.push(false);
        }
    }

    /// Set the element count, resizing every attribute and group to match.
    fn set_len(&mut self, len: usize) {
        self.len = len;
        for a in self.attribs.values_mut() {
            a.resize(len);
        }
        for g in self.groups.values_mut() {
            g.resize(len, false);
        }
    }

    /// Rebuild the store around a new element order: element `n` of the result
    /// is element `idx[n]` of this one. See [`AttribData::gather`].
    fn gather(&self, idx: &[u32]) -> AttribStore {
        let attribs = self
            .attribs
            .iter()
            .map(|(k, v)| (k.clone(), v.gather(idx)))
            .collect();
        let groups = self
            .groups
            .iter()
            .map(|(k, v)| {
                let picked = idx
                    .iter()
                    .map(|&i| v.get(i as usize).copied().unwrap_or(false))
                    .collect();
                (k.clone(), picked)
            })
            .collect();
        AttribStore { len: idx.len(), attribs, kinds: self.kinds.clone(), groups }
    }

    /// Append `other`'s elements. Attributes present on only one side are
    /// created on the other and zero-filled there, so a merge never silently
    /// drops a column.
    fn append(&mut self, other: &AttribStore) {
        let (lhs_len, rhs_len) = (self.len, other.len);

        for (name, rhs) in &other.attribs {
            match self.attribs.get_mut(name) {
                Some(lhs) if lhs.ty() == rhs.ty() => append_data(lhs, rhs),
                // A type clash keeps the left side and zero-fills: the
                // alternative is dropping one side's values entirely, and a
                // merge is not the place to decide which side is right.
                Some(lhs) => lhs.resize(lhs_len + rhs_len),
                None => {
                    let mut fresh = AttribData::zeroed(rhs.ty(), lhs_len);
                    append_data(&mut fresh, rhs);
                    self.attribs.insert(name.clone(), fresh);
                }
            }
        }
        for (_, lhs) in self.attribs.iter_mut().filter(|(n, _)| !other.attribs.contains_key(*n)) {
            lhs.resize(lhs_len + rhs_len);
        }

        // A kind declared on either side sticks: the left side wins a
        // disagreement, the same way its values do.
        for (name, kind) in &other.kinds {
            self.kinds.entry(name.clone()).or_insert(*kind);
        }

        for (name, rhs) in &other.groups {
            let lhs = self
                .groups
                .entry(name.clone())
                .or_insert_with(|| vec![false; lhs_len]);
            lhs.extend_from_slice(rhs);
        }
        for (_, lhs) in self.groups.iter_mut().filter(|(n, _)| !other.groups.contains_key(*n)) {
            lhs.resize(lhs_len + rhs_len, false);
        }

        self.len = lhs_len + rhs_len;
    }
}

const DETAIL_MAGIC: &[u8; 8] = b"CCEDTL01";

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// A bounds-checked cursor over a blob. Every read either yields the bytes it
/// promised or fails; nothing here can index past the buffer.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("length overflow")?;
        let slice = self.b.get(self.at..end).ok_or("unexpected end of blob")?;
        self.at = end;
        Ok(slice)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "attribute name is not UTF-8".to_string())
    }
}

impl AttribStore {
    fn write_into(&self, out: &mut Vec<u8>) {
        put_u32(out, self.len as u32);
        let names = self.names();
        put_u32(out, names.len() as u32);
        for name in names {
            let data = &self.attribs[name];
            put_str(out, name);
            out.push(match data.ty() {
                AttribType::Float => 0,
                AttribType::Float2 => 1,
                AttribType::Float3 => 2,
                AttribType::Float4 => 3,
                AttribType::Int => 4,
            });
            out.push(match self.kind(name) {
                AttribKind::Live => 0,
                AttribKind::Derivative => 1,
            });
            match data {
                AttribData::Float(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
                AttribData::Float2(v) => {
                    out.extend(v.iter().flatten().flat_map(|x| x.to_le_bytes()))
                }
                AttribData::Float3(v) => {
                    out.extend(v.iter().flatten().flat_map(|x| x.to_le_bytes()))
                }
                AttribData::Float4(v) => {
                    out.extend(v.iter().flatten().flat_map(|x| x.to_le_bytes()))
                }
                AttribData::Int(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
            }
        }
        let groups = self.group_names();
        put_u32(out, groups.len() as u32);
        for name in groups {
            put_str(out, name);
            out.extend(self.groups[name].iter().map(|&m| m as u8));
        }
    }

    fn read_from(r: &mut Reader) -> Result<AttribStore, String> {
        let len = r.u32()? as usize;
        let mut store = AttribStore::with_len(len);
        let n_attrs = r.u32()? as usize;
        for _ in 0..n_attrs {
            let name = r.str()?;
            let ty = match r.take(1)?[0] {
                0 => AttribType::Float,
                1 => AttribType::Float2,
                2 => AttribType::Float3,
                3 => AttribType::Float4,
                4 => AttribType::Int,
                other => return Err(format!("unknown attribute type {other}")),
            };
            let kind = match r.take(1)?[0] {
                0 => AttribKind::Live,
                1 => AttribKind::Derivative,
                other => return Err(format!("unknown attribute kind {other}")),
            };
            let data = match ty {
                AttribType::Int => {
                    let mut v = Vec::with_capacity(len.min(1 << 20));
                    for _ in 0..len {
                        v.push(r.i32()?);
                    }
                    AttribData::Int(v)
                }
                _ => {
                    let k = ty.components();
                    let mut flat = Vec::with_capacity((len * k).min(1 << 22));
                    for _ in 0..len * k {
                        flat.push(r.f32()?);
                    }
                    match ty {
                        AttribType::Float => AttribData::Float(flat),
                        AttribType::Float2 => {
                            AttribData::Float2(flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
                        }
                        AttribType::Float3 => AttribData::Float3(
                            flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
                        ),
                        _ => AttribData::Float4(
                            flat.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect(),
                        ),
                    }
                }
            };
            store.attribs.insert(name.clone(), data);
            if kind != AttribKind::Live {
                store.kinds.insert(name, kind);
            }
        }
        let n_groups = r.u32()? as usize;
        for _ in 0..n_groups {
            let name = r.str()?;
            let bits = r.take(len)?;
            store.groups.insert(name, bits.iter().map(|&b| b != 0).collect());
        }
        Ok(store)
    }
}

fn append_data(lhs: &mut AttribData, rhs: &AttribData) {
    match (lhs, rhs) {
        (AttribData::Float(a), AttribData::Float(b)) => a.extend_from_slice(b),
        (AttribData::Float2(a), AttribData::Float2(b)) => a.extend_from_slice(b),
        (AttribData::Float3(a), AttribData::Float3(b)) => a.extend_from_slice(b),
        (AttribData::Float4(a), AttribData::Float4(b)) => a.extend_from_slice(b),
        (AttribData::Int(a), AttribData::Int(b)) => a.extend_from_slice(b),
        (lhs, rhs) => lhs.resize(lhs.len() + rhs.len()),
    }
}

/// Derived connectivity: which primitives use a point, which points share an
/// edge with it, and the unique edge list.
///
/// Built on demand by [`Detail::topology`] and dropped by any structural edit.
/// Everything is CSR — a start-offset array indexed by point, plus one flat
/// array of contents — so a neighbour walk is a slice, not an allocation, and
/// the whole thing uploads to a GPU buffer unchanged in Phase 1.
#[derive(Clone, Debug, Default)]
pub struct Topology {
    point_prim_start: Vec<u32>,
    point_prim: Vec<u32>,
    point_nbr_start: Vec<u32>,
    point_nbr: Vec<u32>,
    edges: Vec<[u32; 2]>,
}

impl Topology {
    /// The primitives using point `p`, ascending.
    pub fn point_prims(&self, p: usize) -> &[u32] {
        Self::span(&self.point_prim_start, &self.point_prim, p)
    }

    /// The points sharing an edge with point `p`, ascending and deduplicated.
    pub fn point_neighbours(&self, p: usize) -> &[u32] {
        Self::span(&self.point_nbr_start, &self.point_nbr, p)
    }

    /// Every unique undirected edge, each as `[low, high]`.
    pub fn edges(&self) -> &[[u32; 2]] {
        &self.edges
    }

    /// How many edges meet at point `p` — Houdini's valence, and the number
    /// incremental remeshing steers toward 6.
    pub fn valence(&self, p: usize) -> usize {
        self.point_neighbours(p).len()
    }

    fn span<'a>(start: &[u32], flat: &'a [u32], i: usize) -> &'a [u32] {
        if i + 1 >= start.len() {
            return &[];
        }
        let (a, b) = (start[i] as usize, start[i + 1] as usize);
        flat.get(a..b).unwrap_or(&[])
    }

    fn build(num_points: usize, vert_point: &[u32], prim_start: &[u32]) -> Topology {
        let num_prims = prim_start.len().saturating_sub(1);

        // point -> prims, by counting sort: one pass to count, a prefix sum,
        // then one pass to place. A point appearing twice in one primitive
        // (a degenerate fan) is counted once.
        let mut counts = vec![0u32; num_points + 1];
        let mut seen: Vec<u32> = Vec::new();
        for prim in 0..num_prims {
            seen.clear();
            for &pt in &vert_point[prim_start[prim] as usize..prim_start[prim + 1] as usize] {
                if !seen.contains(&pt) {
                    seen.push(pt);
                    if (pt as usize) < num_points {
                        counts[pt as usize] += 1;
                    }
                }
            }
        }
        let mut point_prim_start = vec![0u32; num_points + 1];
        let mut acc = 0u32;
        for p in 0..num_points {
            point_prim_start[p] = acc;
            acc += counts[p];
        }
        point_prim_start[num_points] = acc;

        let mut cursor = point_prim_start.clone();
        let mut point_prim = vec![0u32; acc as usize];
        for prim in 0..num_prims {
            seen.clear();
            for &pt in &vert_point[prim_start[prim] as usize..prim_start[prim + 1] as usize] {
                if !seen.contains(&pt) {
                    seen.push(pt);
                    if (pt as usize) < num_points {
                        point_prim[cursor[pt as usize] as usize] = prim as u32;
                        cursor[pt as usize] += 1;
                    }
                }
            }
        }

        // Edges: every consecutive pair around each primitive, closing the
        // loop. A two-point primitive (an open line segment) contributes one
        // edge, not two — closing it would invent a neighbour.
        let mut edges: Vec<[u32; 2]> = Vec::new();
        for prim in 0..num_prims {
            let pts = &vert_point[prim_start[prim] as usize..prim_start[prim + 1] as usize];
            let n = pts.len();
            if n < 2 {
                continue;
            }
            let span = if n == 2 { 1 } else { n };
            for i in 0..span {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                if a == b {
                    continue;
                }
                edges.push([a.min(b), a.max(b)]);
            }
        }
        edges.sort_unstable();
        edges.dedup();

        // point -> neighbours, from the deduplicated edge list. Both endpoints
        // of every edge, counting-sorted the same way.
        let mut counts = vec![0u32; num_points];
        for e in &edges {
            for &p in e {
                if (p as usize) < num_points {
                    counts[p as usize] += 1;
                }
            }
        }
        let mut point_nbr_start = vec![0u32; num_points + 1];
        let mut acc = 0u32;
        for p in 0..num_points {
            point_nbr_start[p] = acc;
            acc += counts[p];
        }
        point_nbr_start[num_points] = acc;

        let mut cursor = point_nbr_start.clone();
        let mut point_nbr = vec![0u32; acc as usize];
        for e in &edges {
            let (a, b) = (e[0], e[1]);
            if (a as usize) < num_points {
                point_nbr[cursor[a as usize] as usize] = b;
                cursor[a as usize] += 1;
            }
            if (b as usize) < num_points {
                point_nbr[cursor[b as usize] as usize] = a;
                cursor[b as usize] += 1;
            }
        }
        for p in 0..num_points {
            let (a, b) = (point_nbr_start[p] as usize, point_nbr_start[p + 1] as usize);
            point_nbr[a..b].sort_unstable();
        }

        Topology { point_prim_start, point_prim, point_nbr_start, point_nbr, edges }
    }
}

/// Points, vertices, primitives and detail — one piece of geometry.
///
/// See the module docs. Position and [`PointId`] get dedicated fields rather
/// than living in the point attribute store: every operator touches both, and
/// neither should cost a name lookup or be removable.
#[derive(Debug)]
pub struct Detail {
    pos: Vec<[f32; 3]>,
    ids: Vec<PointId>,
    next_id: PointId,
    points: AttribStore,
    /// One entry per vertex: the point it references. Primitives index into
    /// this array through `prim_start`.
    vert_point: Vec<u32>,
    verts: AttribStore,
    /// CSR offsets into `vert_point`, one per primitive plus a trailing total.
    /// Always non-empty: a geometry with no primitives still has `[0]`.
    prim_start: Vec<u32>,
    prims: AttribStore,
    detail: AttribStore,
    topo: OnceLock<Topology>,
}

impl Default for Detail {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Detail {
    /// The topology cache is deliberately *not* cloned. It is derived, the
    /// clone exists to be modified, and rebuilding is cheaper than reasoning
    /// about whether a stale cache came along.
    fn clone(&self) -> Self {
        Self {
            pos: self.pos.clone(),
            ids: self.ids.clone(),
            next_id: self.next_id,
            points: self.points.clone(),
            vert_point: self.vert_point.clone(),
            verts: self.verts.clone(),
            prim_start: self.prim_start.clone(),
            prims: self.prims.clone(),
            detail: self.detail.clone(),
            topo: OnceLock::new(),
        }
    }
}

impl Detail {
    pub fn new() -> Self {
        Self {
            pos: Vec::new(),
            ids: Vec::new(),
            next_id: 0,
            points: AttribStore::default(),
            vert_point: Vec::new(),
            verts: AttribStore::default(),
            prim_start: vec![0],
            prims: AttribStore::default(),
            detail: AttribStore::with_len(1),
            topo: OnceLock::new(),
        }
    }

    // ---- counts ----

    pub fn num_points(&self) -> usize {
        self.pos.len()
    }

    pub fn num_verts(&self) -> usize {
        self.vert_point.len()
    }

    pub fn num_prims(&self) -> usize {
        self.prim_start.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    // ---- attribute stores ----

    pub fn points(&self) -> &AttribStore {
        &self.points
    }

    pub fn points_mut(&mut self) -> &mut AttribStore {
        &mut self.points
    }

    pub fn verts(&self) -> &AttribStore {
        &self.verts
    }

    pub fn verts_mut(&mut self) -> &mut AttribStore {
        &mut self.verts
    }

    pub fn prims(&self) -> &AttribStore {
        &self.prims
    }

    pub fn prims_mut(&mut self) -> &mut AttribStore {
        &mut self.prims
    }

    /// The single-row detail store — where whole-geometry values live.
    pub fn detail(&self) -> &AttribStore {
        &self.detail
    }

    pub fn detail_mut(&mut self) -> &mut AttribStore {
        &mut self.detail
    }

    pub fn store(&self, class: Class) -> &AttribStore {
        match class {
            Class::Point => &self.points,
            Class::Vertex => &self.verts,
            Class::Prim => &self.prims,
            Class::Detail => &self.detail,
        }
    }

    pub fn store_mut(&mut self, class: Class) -> &mut AttribStore {
        match class {
            Class::Point => &mut self.points,
            Class::Vertex => &mut self.verts,
            Class::Prim => &mut self.prims,
            Class::Detail => &mut self.detail,
        }
    }

    // ---- points ----

    pub fn pos(&self, p: usize) -> Vec3 {
        self.pos.get(p).map(|v| Vec3::from(*v)).unwrap_or(Vec3::ZERO)
    }

    pub fn set_pos(&mut self, p: usize, v: Vec3) {
        if let Some(slot) = self.pos.get_mut(p) {
            *slot = v.to_array();
        }
    }

    /// Every position, flat — the slice a GPU buffer takes directly.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.pos
    }

    /// Positions for in-place editing. Moving points does not change topology,
    /// so the cache survives; a caller that adds or removes points must go
    /// through [`Detail::add_point`] or [`Detail::gather_points`] instead.
    pub fn positions_mut(&mut self) -> &mut [[f32; 3]] {
        &mut self.pos
    }

    /// The stable identity of point `p`.
    pub fn id(&self, p: usize) -> Option<PointId> {
        self.ids.get(p).copied()
    }

    pub fn ids(&self) -> &[PointId] {
        &self.ids
    }

    /// Where the point carrying `id` currently sits, or `None` if it is gone.
    /// Linear; a solver resolving many ids at once should build a map with
    /// [`Detail::id_map`] instead.
    pub fn index_of_id(&self, id: PointId) -> Option<usize> {
        self.ids.iter().position(|&i| i == id)
    }

    /// Identity to index, for the solver path that reconciles two frames.
    pub fn id_map(&self) -> HashMap<PointId, u32> {
        self.ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as u32))
            .collect()
    }

    /// Replace every point's identity, and the counter new points draw from.
    ///
    /// For a rebuild that KNOWS which points it preserved — the remesher,
    /// which tears a mesh apart and puts it back, and needs the survivors to
    /// come out as themselves. Everything else must let [`Detail::add_point`]
    /// allocate, or two points end up answering to one identity.
    ///
    /// A mismatched length is refused rather than padded: a partial identity
    /// map is worse than none, because the points it does map look right.
    pub fn set_ids(&mut self, ids: Vec<PointId>, next_id: PointId) -> Result<(), String> {
        if ids.len() != self.pos.len() {
            return Err(format!(
                "{} identities for {} points",
                ids.len(),
                self.pos.len()
            ));
        }
        self.next_id = next_id.max(ids.iter().copied().max().map(|m| m + 1).unwrap_or(0));
        self.ids = ids;
        Ok(())
    }

    /// Add a point at `pos`, assigning it a fresh identity. Returns its index.
    pub fn add_point(&mut self, pos: Vec3) -> u32 {
        let idx = self.pos.len() as u32;
        self.pos.push(pos.to_array());
        self.ids.push(self.next_id);
        self.next_id += 1;
        self.points.push_element();
        self.invalidate();
        idx
    }

    /// Add `n` points at once, which is what a generator does. Returns the
    /// index of the first.
    pub fn add_points(&mut self, positions: &[[f32; 3]]) -> u32 {
        let first = self.pos.len() as u32;
        self.pos.extend_from_slice(positions);
        for _ in 0..positions.len() {
            self.ids.push(self.next_id);
            self.next_id += 1;
        }
        self.points.set_len(self.pos.len());
        self.invalidate();
        first
    }

    // ---- primitives ----

    /// Add a primitive over the given points, in winding order. One vertex per
    /// entry. Returns the primitive index.
    pub fn add_prim(&mut self, points: &[u32]) -> u32 {
        let idx = self.num_prims() as u32;
        self.vert_point.extend_from_slice(points);
        self.prim_start.push(self.vert_point.len() as u32);
        self.verts.set_len(self.vert_point.len());
        self.prims.push_element();
        self.invalidate();
        idx
    }

    /// The vertex indices of primitive `p`.
    pub fn prim_verts(&self, p: usize) -> std::ops::Range<usize> {
        if p + 1 >= self.prim_start.len() {
            return 0..0;
        }
        self.prim_start[p] as usize..self.prim_start[p + 1] as usize
    }

    /// The points of primitive `p`, in winding order.
    pub fn prim_points(&self, p: usize) -> &[u32] {
        let r = self.prim_verts(p);
        self.vert_point.get(r).unwrap_or(&[])
    }

    /// The point a vertex references.
    pub fn vert_point(&self, v: usize) -> Option<u32> {
        self.vert_point.get(v).copied()
    }

    pub fn vert_points(&self) -> &[u32] {
        &self.vert_point
    }

    // ---- topology ----

    /// Connectivity, built on first ask and reused until a structural edit
    /// drops it.
    pub fn topology(&self) -> &Topology {
        self.topo
            .get_or_init(|| Topology::build(self.num_points(), &self.vert_point, &self.prim_start))
    }

    /// The points sharing an edge with point `p`.
    pub fn point_neighbours(&self, p: usize) -> &[u32] {
        self.topology().point_neighbours(p)
    }

    /// The primitives using point `p`.
    pub fn point_prims(&self, p: usize) -> &[u32] {
        self.topology().point_prims(p)
    }

    /// Every unique undirected edge.
    pub fn edges(&self) -> &[[u32; 2]] {
        self.topology().edges()
    }

    /// Drop the derived topology. Called by every structural edit; public
    /// because an operator writing `vert_point` through a future bulk path
    /// must be able to say so.
    pub fn invalidate(&mut self) {
        self.topo.take();
    }

    // ---- bulk edits ----

    /// Rebuild around a new point order: point `n` of the result is point
    /// `idx[n]` of this one. Identities, positions, point attributes and point
    /// groups all follow. Primitives are rewired through the inverse map, and
    /// any primitive referencing a dropped point is dropped with it — a
    /// half-referenced polygon is not geometry.
    pub fn gather_points(&mut self, idx: &[u32]) {
        let mut inverse = vec![u32::MAX; self.num_points()];
        for (new, &old) in idx.iter().enumerate() {
            if let Some(slot) = inverse.get_mut(old as usize) {
                // A point appearing twice keeps its first landing place; the
                // duplicate still exists, it is simply not what primitives
                // point at.
                if *slot == u32::MAX {
                    *slot = new as u32;
                }
            }
        }

        self.pos = idx
            .iter()
            .map(|&i| self.pos.get(i as usize).copied().unwrap_or([0.0; 3]))
            .collect();
        self.ids = idx
            .iter()
            .map(|&i| self.ids.get(i as usize).copied().unwrap_or(0))
            .collect();
        self.points = self.points.gather(idx);

        let mut vert_point = Vec::with_capacity(self.vert_point.len());
        let mut prim_start = vec![0u32];
        let mut kept_prims: Vec<u32> = Vec::new();
        let mut kept_verts: Vec<u32> = Vec::new();
        for prim in 0..self.num_prims() {
            let range = self.prim_verts(prim);
            let survives = self.vert_point[range.clone()]
                .iter()
                .all(|&pt| inverse.get(pt as usize).copied().unwrap_or(u32::MAX) != u32::MAX);
            if !survives {
                continue;
            }
            for v in range {
                kept_verts.push(v as u32);
                vert_point.push(inverse[self.vert_point[v] as usize]);
            }
            prim_start.push(vert_point.len() as u32);
            kept_prims.push(prim as u32);
        }

        self.verts = self.verts.gather(&kept_verts);
        self.prims = self.prims.gather(&kept_prims);
        self.vert_point = vert_point;
        self.prim_start = prim_start;
        self.invalidate();
    }

    /// Merge points onto representatives: point `p` becomes `rep[p]`.
    ///
    /// A representative keeps its identity and its values — the same choice
    /// the remesher's collapse makes, and for the same reason: one of the two
    /// is a point the solver has been writing to, and the merge should cost
    /// the simulation as little memory as it can. Primitives are rewired, and
    /// one left with a repeated corner is dropped, because a triangle with two
    /// corners in the same place is not a triangle.
    ///
    /// Chains are followed, so `rep` need not already be flat: a fuse that
    /// pointed a at b and b at c leaves everything at c.
    pub fn fuse_points(&mut self, rep: &[u32]) {
        let n = self.num_points();
        let root = |mut p: u32| {
            // Bounded rather than trusting the map to be acyclic: a cycle in a
            // caller's representative map would otherwise hang the app.
            for _ in 0..n {
                let next = rep.get(p as usize).copied().unwrap_or(p);
                if next == p {
                    break;
                }
                p = next;
            }
            p
        };
        for v in self.vert_point.iter_mut() {
            *v = root(*v);
        }

        // A primitive whose corners collapsed onto each other is not a
        // primitive any more. Dropped here rather than left for the point
        // compaction, which only knows about points that went away — these
        // ones all still exist, they have just stopped being distinct.
        let mut vert_point = Vec::with_capacity(self.vert_point.len());
        let mut prim_start = vec![0u32];
        let mut kept_prims: Vec<u32> = Vec::new();
        let mut kept_verts: Vec<u32> = Vec::new();
        for prim in 0..self.num_prims() {
            let range = self.prim_verts(prim);
            let pts = &self.vert_point[range.clone()];
            let mut uniq = pts.to_vec();
            uniq.sort_unstable();
            uniq.dedup();
            if uniq.len() < pts.len() || uniq.len() < 3 {
                continue;
            }
            for v in range {
                kept_verts.push(v as u32);
                vert_point.push(self.vert_point[v]);
            }
            prim_start.push(vert_point.len() as u32);
            kept_prims.push(prim as u32);
        }
        self.verts = self.verts.gather(&kept_verts);
        self.prims = self.prims.gather(&kept_prims);
        self.vert_point = vert_point;
        self.prim_start = prim_start;

        let keep: Vec<bool> = (0..n).map(|p| root(p as u32) as usize == p).collect();
        self.invalidate();
        self.keep_points(&keep);
    }

    /// Keep the points `keep` marks true, dropping the rest.
    pub fn keep_points(&mut self, keep: &[bool]) {
        let idx: Vec<u32> = (0..self.num_points() as u32)
            .filter(|&i| keep.get(i as usize).copied().unwrap_or(false))
            .collect();
        self.gather_points(&idx);
    }

    /// Append `other`. Identities are reallocated on the way in, so two pieces
    /// of geometry that were generated independently — and therefore both
    /// number their points from zero — do not collide.
    pub fn merge(&mut self, other: &Detail) {
        let point_offset = self.num_points() as u32;
        let vert_offset = self.vert_point.len() as u32;

        self.pos.extend_from_slice(&other.pos);
        for _ in 0..other.num_points() {
            self.ids.push(self.next_id);
            self.next_id += 1;
        }
        self.points.append(&other.points);

        self.vert_point
            .extend(other.vert_point.iter().map(|&p| p + point_offset));
        self.verts.append(&other.verts);

        for w in other.prim_start.iter().skip(1) {
            self.prim_start.push(w + vert_offset);
        }
        self.prims.append(&other.prims);

        self.invalidate();
    }

    // ---- convenience ----

    /// Point color, from `Cd` where it exists.
    pub fn color(&self, p: usize) -> [f32; 3] {
        match self.points.value(CD, p) {
            Some(AttribValue::Float3(c)) => c,
            Some(other) => other.as_vec3().to_array(),
            None => DEFAULT_COLOR,
        }
    }

    /// Set point color, creating `Cd` if this is the first writer.
    pub fn set_color(&mut self, p: usize, c: [f32; 3]) {
        self.points
            .get_or_create(CD, AttribValue::Float3(DEFAULT_COLOR));
        let _ = self.points.set_value(CD, p, AttribValue::Float3(c));
    }

    /// Whether the surface is closed: every directed edge has exactly one
    /// opposite.
    ///
    /// The question "what is inside this?" only has an answer for a closed
    /// surface. A flat disc, a torn mesh or a single polygon has no inside, and
    /// anything that signs a distance field has to know the difference — sign
    /// an open surface and you get whichever side its normals happen to face,
    /// which is not a solid, just a preference.
    ///
    /// Directed, not undirected, because the property that matters is "no
    /// boundary, consistently wound", and those are the same test: every edge
    /// walked one way by one face and the other way by its neighbour. Counting
    /// undirected edges instead would ask for exactly two faces per edge, which
    /// is [`is_manifold`](Self::is_manifold) — a stricter thing that a boolean
    /// legitimately fails where two sheets pinch together along a knife edge
    /// thinner than a voxel. Such a surface is still watertight, still has an
    /// inside, and still voxelizes correctly.
    pub fn is_closed(&self) -> bool {
        if self.num_prims() == 0 {
            return false;
        }
        let mut counts: HashMap<[u32; 2], i32> = HashMap::new();
        for prim in 0..self.num_prims() {
            let pts = self.prim_points(prim);
            if pts.len() < 3 {
                return false;
            }
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                // One counter per undirected edge, incremented one way and
                // decremented the other: it lands on zero exactly when every
                // traversal is matched by an opposite one.
                let (key, step) = if a < b { ([a, b], 1) } else { ([b, a], -1) };
                *counts.entry(key).or_default() += step;
            }
        }
        counts.values().all(|&c| c == 0)
    }

    /// Whether every edge is shared by exactly two primitives.
    ///
    /// Stricter than [`is_closed`](Self::is_closed): it also rules out the
    /// place where more than two faces meet along one edge. Remeshing wants
    /// this — an edge with four faces has no single pair to flip or collapse
    /// between — while voxelizing does not.
    pub fn is_manifold(&self) -> bool {
        if self.num_prims() == 0 {
            return false;
        }
        let mut counts: HashMap<[u32; 2], usize> = HashMap::new();
        for prim in 0..self.num_prims() {
            let pts = self.prim_points(prim);
            if pts.len() < 3 {
                return false;
            }
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                *counts.entry([a.min(b), a.max(b)]).or_default() += 1;
            }
        }
        counts.values().all(|&c| c == 2)
    }

    /// The axis-aligned bounds, or `None` when there are no points.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.pos.first()?;
        let (mut lo, mut hi) = (Vec3::from(first), Vec3::from(first));
        for p in &self.pos[1..] {
            let v = Vec3::from(*p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        Some((lo, hi))
    }

    /// Zero every Derivative attribute on every class — the step boundary's
    /// first act. See [`AttribKind`].
    pub fn clear_derivatives(&mut self) {
        self.points.clear_derivatives();
        self.verts.clear_derivatives();
        self.prims.clear_derivatives();
        self.detail.clear_derivatives();
    }

    /// Restore this geometry's Live point attributes from `prev`, matching
    /// points by identity.
    ///
    /// The other half of the contract, and the one that makes a rebuild safe
    /// to put in the middle of a solve. When a step's chain hands back geometry
    /// that has lost an attribute — a kernel generator that rebuilt its points,
    /// and in Phase 3 a remesh — the values are not gone, they are in the
    /// previous state, attached to identities. A point that survived gets its
    /// value back; a point that is genuinely new gets the type's zero, which is
    /// the only honest answer for a place that did not exist last step.
    ///
    /// Attributes the new geometry DOES carry are left alone: the chain
    /// computed them this step and that is the whole point of running it.
    /// Derivative attributes are not restored at all — they are meant to be
    /// rebuilt, and carrying one across would be exactly the silent
    /// accumulation the kind exists to prevent.
    pub fn restore_live_from(&mut self, prev: &Detail) {
        // Restoration bridges a REBUILD, not a deletion. If the chain handed
        // back the same identities in the same order, it kept the geometry it
        // was given — so an attribute that is gone was taken out on purpose,
        // and putting it back would override the author. Only when the point
        // set itself changed underneath is a missing attribute evidence of
        // loss rather than intent.
        if self.ids == prev.ids {
            return;
        }
        let missing: Vec<&str> = prev
            .points
            .names_of_kind(AttribKind::Live)
            .into_iter()
            .filter(|n| !self.points.has(n))
            .collect();
        if missing.is_empty() {
            return;
        }
        let was: HashMap<PointId, u32> = prev.id_map();
        for name in missing {
            let Some(src) = prev.points.get(name) else { continue };
            let ty = src.ty();
            let mut data = AttribData::zeroed(ty, self.num_points());
            for p in 0..self.num_points() {
                let Some(id) = self.id(p) else { continue };
                let Some(&old) = was.get(&id) else { continue };
                if let Some(v) = src.get(old as usize) {
                    let _ = data.set(p, v);
                }
            }
            let _ = self.points.insert(name, data);
            self.points.set_kind(name, AttribKind::Live);
        }
    }

    /// Serialize to a compact binary blob.
    ///
    /// Hand-rolled rather than derived, because the one thing a cache is for is
    /// being cheaper than recomputing: a hundred thousand points of JSON text
    /// is not. Positions and attribute arrays go out as raw little-endian
    /// floats, which is also how they sit in memory.
    ///
    /// The derived topology is NOT written — it is rebuilt from the primitives
    /// on read, and storing it would mean a file that can disagree with itself.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(DETAIL_MAGIC);
        put_u32(&mut out, self.pos.len() as u32);
        put_u64(&mut out, self.next_id);
        for p in &self.pos {
            for c in p {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        for id in &self.ids {
            put_u64(&mut out, *id);
        }
        put_u32(&mut out, self.vert_point.len() as u32);
        for v in &self.vert_point {
            put_u32(&mut out, *v);
        }
        put_u32(&mut out, self.prim_start.len() as u32);
        for v in &self.prim_start {
            put_u32(&mut out, *v);
        }
        for store in [&self.points, &self.verts, &self.prims, &self.detail] {
            store.write_into(&mut out);
        }
        out
    }

    /// Read back a blob written by [`Detail::to_bytes`].
    ///
    /// Every length is checked against what is actually left in the buffer, so
    /// a truncated or corrupt cache file is an error rather than a huge
    /// allocation or a panic. A cache lives in a directory anything can write
    /// to, and must never be trusted the way a value from memory is.
    pub fn from_bytes(bytes: &[u8]) -> Result<Detail, String> {
        let mut r = Reader { b: bytes, at: 0 };
        if r.take(DETAIL_MAGIC.len())? != DETAIL_MAGIC {
            return Err("not a Detail blob".into());
        }
        let num_points = r.u32()? as usize;
        let next_id = r.u64()?;
        let mut pos = Vec::with_capacity(num_points.min(1 << 20));
        for _ in 0..num_points {
            pos.push([r.f32()?, r.f32()?, r.f32()?]);
        }
        let mut ids = Vec::with_capacity(pos.len());
        for _ in 0..num_points {
            ids.push(r.u64()?);
        }
        let nv = r.u32()? as usize;
        let mut vert_point = Vec::with_capacity(nv.min(1 << 20));
        for _ in 0..nv {
            vert_point.push(r.u32()?);
        }
        let ns = r.u32()? as usize;
        let mut prim_start = Vec::with_capacity(ns.min(1 << 20));
        for _ in 0..ns {
            prim_start.push(r.u32()?);
        }
        if prim_start.is_empty() {
            return Err("primitive offsets are missing their terminator".into());
        }
        let points = AttribStore::read_from(&mut r)?;
        let verts = AttribStore::read_from(&mut r)?;
        let prims = AttribStore::read_from(&mut r)?;
        let detail = AttribStore::read_from(&mut r)?;

        // Cross-checks, because every reader below indexes on these being
        // consistent and a corrupt file must not reach that code.
        if points.len() != num_points || verts.len() != nv || prims.len() != ns - 1 {
            return Err("element counts disagree with their attribute stores".into());
        }
        if vert_point.iter().any(|&p| p as usize >= num_points.max(1)) && num_points > 0 {
            return Err("a vertex references a point that is not there".into());
        }
        Ok(Detail {
            pos,
            ids,
            next_id,
            points,
            vert_point,
            verts,
            prim_start,
            prims,
            detail,
            topo: OnceLock::new(),
        })
    }

    /// Weld a triangle soup into points and triangles: coincident positions
    /// become one point, every three positions become one primitive.
    ///
    /// The migration path for generators that still emit soup, and a faithful
    /// port of `geometry::weld_points` — including its 1e-4 quantization, so
    /// that vertices a kernel emitted from the same formula weld reliably.
    /// Color is carried onto `Cd`, taking the first copy of each welded point.
    pub fn from_triangle_soup(positions: &[[f32; 3]], colors: &[[f32; 3]]) -> Detail {
        Self::from_triangle_soup_with_map(positions, colors).0
    }

    /// [`Detail::from_triangle_soup`], plus the point each input corner welded
    /// onto — so a caller holding per-corner data can carry it across.
    pub fn from_triangle_soup_with_map(
        positions: &[[f32; 3]],
        colors: &[[f32; 3]],
    ) -> (Detail, Vec<u32>) {
        let mut detail = Detail::new();
        let mut key_to_point: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut point_of: Vec<u32> = Vec::with_capacity(positions.len());
        let mut cd: Vec<[f32; 3]> = Vec::new();

        for (i, p) in positions.iter().enumerate() {
            let key = (
                (p[0] as f64 * 1e4).round() as i64,
                (p[1] as f64 * 1e4).round() as i64,
                (p[2] as f64 * 1e4).round() as i64,
            );
            let idx = match key_to_point.get(&key) {
                Some(&idx) => idx,
                None => {
                    let idx = detail.add_point(Vec3::from(*p));
                    key_to_point.insert(key, idx);
                    cd.push(colors.get(i).copied().unwrap_or(DEFAULT_COLOR));
                    idx
                }
            };
            point_of.push(idx);
        }

        for tri in point_of.chunks_exact(3) {
            detail.add_prim(tri);
        }

        if !cd.is_empty() {
            detail
                .points
                .attribs
                .insert(CD.to_string(), AttribData::Float3(cd));
        }
        (detail, point_of)
    }

    /// The point behind every corner [`Detail::triangulate`] emits, in the
    /// same order.
    ///
    /// Lets a caller that had to flatten to triangles — the OpenCL launcher once,
    /// until the Phase 1 ABI binds attributes directly — put results back on
    /// the points they came from instead of welding the output and losing
    /// every identity.
    pub fn triangulate_points(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for prim in 0..self.num_prims() {
            let pts = self.prim_points(prim);
            if pts.len() < 3 {
                continue;
            }
            for i in 1..pts.len() - 1 {
                out.extend_from_slice(&[pts[0], pts[i], pts[i + 1]]);
            }
        }
        out
    }

    /// Fan-triangulate every primitive, handing each corner to `make` as
    /// (position, color).
    ///
    /// The render boundary. It takes a closure rather than returning the
    /// renderer's vertex type so that this module stays free of anything that
    /// draws — the caller in `geometry.rs` supplies `Vertex3D`.
    pub fn triangulate<V>(&self, mut make: impl FnMut([f32; 3], [f32; 3]) -> V) -> Vec<V> {
        let mut out = Vec::new();
        for prim in 0..self.num_prims() {
            let pts = self.prim_points(prim);
            if pts.len() < 3 {
                continue;
            }
            for i in 1..pts.len() - 1 {
                for &p in &[pts[0], pts[i], pts[i + 1]] {
                    let p = p as usize;
                    out.push(make(
                        self.pos.get(p).copied().unwrap_or([0.0; 3]),
                        self.color(p),
                    ));
                }
            }
        }
        out
    }
}
