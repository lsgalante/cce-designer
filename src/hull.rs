//! The convex hull of a point cloud, as a closed triangle mesh — the `hull`
//! node, and what hou-control's `developer_embryo` does with its scattered
//! points (its `shrinkwrap`).
//!
//! The incremental algorithm rather than quickhull: a starting tetrahedron
//! from the extreme points, then each remaining point in turn either lies
//! inside the hull so far or sees some of its faces — those are removed, and
//! the ring of edges where visible meets hidden (the horizon) is fanned to
//! the new point. Points within a tolerance of a face are treated as inside,
//! which is what the HDA's Remove Inline Points does: a hull with a thousand
//! coplanar slivers is not a better hull. It is O(points × faces), which for
//! a thousand scattered points is nothing; a hull of a million would want
//! the conflict lists.

use crate::detail::Detail;
use glam::Vec3;

/// The convex hull of `points`, as a closed triangle mesh over only the
/// points that lie on it — `None` when the points do not span a volume.
pub fn convex_hull(points: &[Vec3]) -> Option<Detail> {
    let faces = hull_faces(points)?;
    let mut remap = vec![u32::MAX; points.len()];
    let mut d = Detail::new();
    for f in &faces {
        let mut ids = [0u32; 3];
        for (k, &pi) in f.iter().enumerate() {
            if remap[pi] == u32::MAX {
                remap[pi] = d.add_point(points[pi]);
            }
            ids[k] = remap[pi];
        }
        d.add_prim(&ids);
    }
    Some(d)
}

/// The hull's faces as index triples into `points`, wound outward.
fn hull_faces(points: &[Vec3]) -> Option<Vec<[usize; 3]>> {
    if points.len() < 4 {
        return None;
    }
    let (lo, hi) = points
        .iter()
        .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let diag = (hi - lo).length();
    if !(diag > 0.0) {
        return None;
    }
    // "On the face" and "no volume" are both judged against the cloud's own
    // size, so the hull of a millimetre embryo and of a metre one build the
    // same way.
    let eps = diag * 1e-5;

    // The starting tetrahedron: the two points furthest apart along an axis,
    // the point furthest from that line, the point furthest from that plane.
    let mut ext = [0usize; 6];
    for (i, p) in points.iter().enumerate() {
        for a in 0..3 {
            if p[a] < points[ext[a]][a] {
                ext[a] = i;
            }
            if p[a] > points[ext[a + 3]][a] {
                ext[a + 3] = i;
            }
        }
    }
    let (mut i0, mut i1, mut best) = (0, 0, -1.0);
    for &a in &ext {
        for &b in &ext {
            let d = (points[a] - points[b]).length();
            if d > best {
                best = d;
                i0 = a;
                i1 = b;
            }
        }
    }
    if best <= eps {
        return None;
    }
    let dir = (points[i1] - points[i0]).normalize();
    let (mut i2, mut best) = (0, -1.0);
    for (i, &p) in points.iter().enumerate() {
        let off = p - points[i0];
        let d = (off - dir * off.dot(dir)).length();
        if d > best {
            best = d;
            i2 = i;
        }
    }
    if best <= eps {
        return None;
    }
    let n = (points[i1] - points[i0]).cross(points[i2] - points[i0]).normalize();
    let (mut i3, mut best) = (0, -1.0);
    for (i, &p) in points.iter().enumerate() {
        let d = (p - points[i0]).dot(n).abs();
        if d > best {
            best = d;
            i3 = i;
        }
    }
    if best <= eps {
        return None;
    }

    // Wind the four faces so every normal points away from the centroid.
    let centroid = (points[i0] + points[i1] + points[i2] + points[i3]) / 4.0;
    let mut faces: Vec<[usize; 3]> = Vec::new();
    for f in [[i0, i1, i2], [i0, i1, i3], [i0, i2, i3], [i1, i2, i3]] {
        let n = face_normal(points, f);
        if (points[f[0]] - centroid).dot(n) < 0.0 {
            faces.push([f[0], f[2], f[1]]);
        } else {
            faces.push(f);
        }
    }

    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (pi, &p) in points.iter().enumerate() {
        if pi == i0 || pi == i1 || pi == i2 || pi == i3 {
            continue;
        }
        // The faces this point looks at from outside.
        let visible: Vec<bool> = faces
            .iter()
            .map(|f| (p - points[f[0]]).dot(face_normal(points, *f)) > eps)
            .collect();
        if !visible.iter().any(|&v| v) {
            continue;
        }
        // The horizon: directed edges of visible faces whose reverse is not
        // an edge of a visible face. The winding of the visible face gives
        // the new face's winding for free.
        edges.clear();
        for (f, &v) in faces.iter().zip(&visible) {
            if v {
                edges.push((f[0], f[1]));
                edges.push((f[1], f[2]));
                edges.push((f[2], f[0]));
            }
        }
        let horizon: Vec<(usize, usize)> =
            edges.iter().copied().filter(|&(a, b)| !edges.contains(&(b, a))).collect();
        let mut kept: Vec<[usize; 3]> =
            faces.iter().zip(&visible).filter(|(_, &v)| !v).map(|(f, _)| *f).collect();
        for (a, b) in horizon {
            kept.push([a, b, pi]);
        }
        faces = kept;
    }
    Some(faces)
}

fn face_normal(points: &[Vec3], f: [usize; 3]) -> Vec3 {
    (points[f[1]] - points[f[0]]).cross(points[f[2]] - points[f[0]]).normalize_or_zero()
}
