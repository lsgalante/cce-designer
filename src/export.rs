//! Writing geometry out: STL and OBJ.
//!
//! Everything the app builds has, until now, been something you could only
//! look at inside it. These are the two formats that matter for what this tool
//! is for: **STL** is what a printer and a mold chain read, and **OBJ** is what
//! every other piece of software reads.
//!
//! ## What each format can carry
//!
//! They are not the same picture of a mesh, and the difference is worth
//! knowing before choosing:
//!
//! - **OBJ keeps the topology.** Points are written once and faces reference
//!   them, so a quad stays a quad and a shared point stays shared. Reimporting
//!   an OBJ gives back the mesh that was exported.
//! - **STL keeps only the triangles.** It has no notion of a shared point: every
//!   triangle carries its own three corners, so a mesh comes back welded-by-
//!   position at best and a quad comes back as two triangles always. That is
//!   the format's design, not a shortcoming of this writer — it exists to feed
//!   a machine that only needs a closed surface.
//!
//! Neither carries attributes. A simulation's state does not survive an
//! export, which is what the project file and the sim cache are for.
//!
//! ## Units
//!
//! Coordinates are written exactly as they are, scaled only by the caller's
//! Scale. The app's World Unit is a DECLARATION about what one unit means, not
//! a conversion (see the Guides node), and export keeps that promise: a
//! geometry modelled at 20 units across writes as 20, and it is the printer's
//! slicer that is told those are millimetres.

use crate::detail::Detail;
use glam::Vec3;

/// The triangles of a piece of geometry, with a face normal each.
///
/// Both STL writers want exactly this, and the fan matches what the viewport
/// and the path tracer draw — so what is exported is what was on screen.
fn triangles(d: &Detail, scale: f32) -> Vec<([Vec3; 3], Vec3)> {
    d.triangulate(|pos, _| Vec3::from(pos) * scale)
        .chunks_exact(3)
        .map(|t| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            ([t[0], t[1], t[2]], n)
        })
        .collect()
}

/// Binary STL: an 80-byte header, a triangle count, then 50 bytes each.
///
/// The header deliberately does NOT begin with "solid" — that word at the
/// start of a file is how readers guess a file is the ASCII form, and a binary
/// file that opens with it is a well-known way to be misread.
pub fn stl_binary(d: &Detail, scale: f32, name: &str) -> Vec<u8> {
    let tris = triangles(d, scale);
    let mut out = Vec::with_capacity(84 + tris.len() * 50);

    let mut header = [0u8; 80];
    let label = format!("cce-designer {name}");
    for (slot, b) in header.iter_mut().zip(label.bytes()) {
        *slot = b;
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());

    for (v, n) in &tris {
        for c in [n.x, n.y, n.z] {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for p in v {
            for c in [p.x, p.y, p.z] {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        // The "attribute byte count", which nothing uses and everything
        // expects to be there.
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// ASCII STL. Bigger and slower to read than the binary form, and the one to
/// reach for when something downstream is being difficult and you want to look
/// at the file.
pub fn stl_ascii(d: &Detail, scale: f32, name: &str) -> String {
    let mut out = format!("solid {name}\n");
    for (v, n) in triangles(d, scale) {
        out.push_str(&format!("  facet normal {:e} {:e} {:e}\n", n.x, n.y, n.z));
        out.push_str("    outer loop\n");
        for p in v {
            out.push_str(&format!("      vertex {:e} {:e} {:e}\n", p.x, p.y, p.z));
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    out.push_str(&format!("endsolid {name}\n"));
    out
}

/// Wavefront OBJ, keeping points shared and faces at their real arity.
///
/// Indices are 1-based, which is the format's convention and the single most
/// common way to write a broken OBJ.
///
/// Normals are written when the geometry carries `N` — the attribute the
/// Normal node publishes — and referenced per corner as `f v//n`. Without it
/// the faces are written bare and the reader computes its own, which is the
/// right default: a stale `N` from before a deform would be worse than none.
pub fn obj(d: &Detail, scale: f32, name: &str) -> String {
    let mut out = format!("# cce-designer {name}\n");
    out.push_str(&format!("o {name}\n"));

    for p in d.positions() {
        out.push_str(&format!("v {:e} {:e} {:e}\n", p[0] * scale, p[1] * scale, p[2] * scale));
    }

    let has_normals = d.points().has("N");
    if has_normals {
        for p in 0..d.num_points() {
            let n = d.points().value("N", p).map(|v| v.as_vec3()).unwrap_or(Vec3::Y);
            out.push_str(&format!("vn {:e} {:e} {:e}\n", n.x, n.y, n.z));
        }
    }

    for prim in 0..d.num_prims() {
        let pts = d.prim_points(prim);
        // A two-point primitive is a line, not a face. OBJ has `l` for exactly
        // this, and writing it as a face would give readers a degenerate
        // triangle to choke on.
        let tag = if pts.len() < 3 { "l" } else { "f" };
        out.push_str(tag);
        for &p in pts {
            let i = p + 1;
            if has_normals && pts.len() >= 3 {
                out.push_str(&format!(" {i}//{i}"));
            } else {
                out.push_str(&format!(" {i}"));
            }
        }
        out.push('\n');
    }
    out
}

/// Which writer a path's extension asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    StlBinary,
    StlAscii,
    Obj,
}

impl Format {
    /// Read off a file name, defaulting to binary STL — the form a printer
    /// wants and the one an unrecognized name most likely meant.
    pub fn from_path(path: &std::path::Path) -> Format {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "obj" => Format::Obj,
            _ => Format::StlBinary,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::StlBinary => "STL",
            Format::StlAscii => "STL (ASCII)",
            Format::Obj => "OBJ",
        }
    }
}

/// Write `geom` to `path`.
///
/// The parent directory is created if it is missing, because being told a
/// directory does not exist is a worse answer than making it, and every other
/// way this app writes a file does the same.
pub fn write(
    geom: &Detail,
    path: &std::path::Path,
    format: Format,
    scale: f32,
) -> Result<usize, String> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("geometry")
        .to_string();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let bytes = match format {
        Format::StlBinary => stl_binary(geom, scale, &name),
        Format::StlAscii => stl_ascii(geom, scale, &name).into_bytes(),
        Format::Obj => obj(geom, scale, &name).into_bytes(),
    };
    let len = bytes.len();
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(len)
}
