//! Mesh export.

use super::*;

/// A two-quad sheet: enough to tell a format that keeps topology from one
/// that does not.
fn sheet() -> Detail {
    let mut d = Detail::new();
    for (x, z) in [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 1.0), (1.0, 1.0), (2.0, 1.0)] {
        d.add_point(Vec3::new(x, 0.0, z));
    }
    d.add_prim(&[0, 3, 4, 1]);
    d.add_prim(&[1, 4, 5, 2]);
    d
}

#[test]
fn test_stl_writes_a_file_a_printer_can_read() {
    use crate::export::stl_binary;
    let d = sheet();
    let b = stl_binary(&d, 1.0, "sheet");

    // 80-byte header, a count, 50 bytes a triangle. Two quads fan to four.
    let count = u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize;
    assert_eq!(count, 4);
    assert_eq!(b.len(), 84 + count * 50);
    // The header must NOT begin with "solid": that word at the start of a
    // file is how readers guess at the ASCII form, and a binary file that
    // opens with it is a well-known way to be misread.
    assert!(!b.starts_with(b"solid"), "binary STL must not look like ASCII");
    assert!(b.starts_with(b"cce-designer sheet"));

    // Every facet normal agrees with its own winding. STL stores both and
    // they can disagree; a slicer that trusts the normal would then see
    // the surface inside out.
    for t in 0..count {
        let at = 84 + t * 50;
        let f: Vec<f32> = (0..12)
            .map(|i| f32::from_le_bytes(b[at + i * 4..at + i * 4 + 4].try_into().unwrap()))
            .collect();
        let n = Vec3::new(f[0], f[1], f[2]);
        let (a, bb, c) = (
            Vec3::new(f[3], f[4], f[5]),
            Vec3::new(f[6], f[7], f[8]),
            Vec3::new(f[9], f[10], f[11]),
        );
        let geo = (bb - a).cross(c - a).normalize();
        assert!(n.dot(geo) > 0.999, "facet {t}: normal {n:?} vs winding {geo:?}");
        // The attribute byte count nothing uses and everything expects.
        assert_eq!(u16::from_le_bytes(b[at + 48..at + 50].try_into().unwrap()), 0);
    }

    // Scale multiplies coordinates and nothing else.
    let big = stl_binary(&d, 10.0, "sheet");
    let vx = |blob: &[u8]| f32::from_le_bytes(blob[96..100].try_into().unwrap());
    assert!((vx(&big) - vx(&b) * 10.0).abs() < 1e-4);

    // Empty geometry is a valid file with no triangles, not a panic.
    let empty = stl_binary(&Detail::new(), 1.0, "nothing");
    assert_eq!(empty.len(), 84);
    assert_eq!(u32::from_le_bytes(empty[80..84].try_into().unwrap()), 0);
}

#[test]
fn test_obj_keeps_the_topology_that_stl_throws_away() {
    use crate::export::{obj, stl_binary};
    let d = sheet();
    let text = obj(&d, 1.0, "sheet");
    let lines: Vec<&str> = text.lines().collect();

    // One vertex per POINT and one face per PRIMITIVE: a quad stays a
    // quad and a shared point stays shared. STL cannot say either — it
    // fans to four triangles carrying twelve loose corners.
    assert_eq!(lines.iter().filter(|l| l.starts_with("v ")).count(), d.num_points());
    let faces: Vec<&&str> = lines.iter().filter(|l| l.starts_with("f ")).collect();
    assert_eq!(faces.len(), d.num_prims());
    for f in &faces {
        assert_eq!(f.split_whitespace().count() - 1, 4, "a quad did not survive: {f}");
    }
    let count = u32::from_le_bytes(stl_binary(&d, 1.0, "s")[80..84].try_into().unwrap());
    assert_eq!(count, 4, "STL fans the same mesh to triangles");

    // Indices are 1-based and in range — the single most common way to
    // write a broken OBJ.
    for f in &faces {
        for tok in f.split_whitespace().skip(1) {
            let i: usize = tok.split('/').next().unwrap().parse().unwrap();
            assert!(i >= 1 && i <= d.num_points(), "index {i} out of range");
        }
    }
}

#[test]
fn test_obj_writes_normals_only_when_the_geometry_has_them() {
    use crate::export::obj;
    let plain = obj(&sheet(), 1.0, "s");
    assert!(!plain.contains("vn "), "normals appeared from nowhere");
    assert!(plain.lines().any(|l| l.starts_with("f 1 4 5 2")), "{plain}");

    // With N present they are written and referenced per corner. Without
    // it the faces stay bare and the reader computes its own, which is the
    // right default: a stale N from before a deform is worse than none.
    let mut d = sheet();
    d.points_mut().create("N", AttribValue::Float3([0.0, 1.0, 0.0]));
    let with = obj(&d, 1.0, "s");
    assert_eq!(with.lines().filter(|l| l.starts_with("vn ")).count(), d.num_points());
    assert!(with.lines().any(|l| l.starts_with("f 1//1 4//4")), "{with}");
}

#[test]
fn test_obj_writes_a_two_point_primitive_as_a_line() {
    use crate::export::obj;
    // Polygon unfilled is a ring of two-point prims. OBJ has `l` for
    // exactly this; writing them as faces would hand readers degenerate
    // triangles.
    let root = modelling_root(
        "1.0",
        vec![phase3_node("polygon", &[("sides", "5"), ("fill", "false")])],
    );
    let (ring, _) = eval_node(&root, "polygon 1");
    let text = obj(&ring, 1.0, "ring");
    assert_eq!(text.lines().filter(|l| l.starts_with("l ")).count(), 5);
    assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 0);
}

#[test]
fn test_ascii_stl_is_the_same_mesh_in_words() {
    use crate::export::{stl_ascii, stl_binary};
    let d = sheet();
    let text = stl_ascii(&d, 1.0, "sheet");
    let facets = text.lines().filter(|l| l.trim_start().starts_with("facet normal")).count();
    let verts = text.lines().filter(|l| l.trim_start().starts_with("vertex")).count();
    let count = u32::from_le_bytes(stl_binary(&d, 1.0, "s")[80..84].try_into().unwrap()) as usize;
    assert_eq!(facets, count);
    assert_eq!(verts, count * 3);
    assert!(text.starts_with("solid sheet\n") && text.trim_end().ends_with("endsolid sheet"));
}

#[test]
fn test_the_extension_picks_the_format() {
    use crate::export::Format;
    use std::path::Path;
    assert_eq!(Format::from_path(Path::new("a/b.obj")), Format::Obj);
    assert_eq!(Format::from_path(Path::new("a/b.OBJ")), Format::Obj);
    assert_eq!(Format::from_path(Path::new("a/b.stl")), Format::StlBinary);
    // Anything unrecognized is binary STL: the form a printer wants, and
    // the one an unlabelled name most likely meant.
    assert_eq!(Format::from_path(Path::new("a/b")), Format::StlBinary);
}

#[test]
fn test_write_creates_the_directory_and_reports_the_size() {
    use crate::export::{write, Format};
    let dir = std::env::temp_dir()
        .join(format!("cce-export-test-{}", std::process::id()))
        .join("nested");
    let path = dir.join("thing.obj");
    let n = write(&sheet(), &path, Format::Obj, 1.0).expect("write");
    assert!(path.exists(), "the nested directory was not created");
    assert_eq!(n, std::fs::metadata(&path).unwrap().len() as usize);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    let _ = std::fs::remove_dir(dir.parent().unwrap());
}
