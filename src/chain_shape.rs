//! The shrink-wrapped outline round a wired chain of the network
//! (`State::chain_shapes`): a region of axis-aligned edges that follows the
//! chain's nodes and wires rather than boxing their bounds.
//!
//! It is the ORTHOGONAL CONVEX HULL of a set of rectangles — the padded
//! nodes and the padded routes of the wires between them — which is what
//! shrink-wrap is when only horizontal and vertical edges are allowed: the
//! smallest region holding the rectangles that every horizontal and every
//! vertical line crosses in one run. So a column of nodes is a column, its
//! right edge stepping with the labels, and an L of nodes is an L, where
//! the bounding box of either would take in the empty corner.
//!
//! Kept on a grid of the rectangles' own edges (coordinate compression): a
//! cell between two consecutive xs and two consecutive ys is in or out.

/// An axis-aligned rectangle as its two corners, `(x0, y0, x1, y1)`.
pub type Span = (f32, f32, f32, f32);

#[derive(Debug, Clone, PartialEq)]
pub struct ChainShape {
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Row-major, `(ys.len() - 1)` rows of `(xs.len() - 1)` cells.
    inside: Vec<bool>,
}

/// Edges nearer than this are one edge: a step of a fraction of a pixel is
/// arithmetic, not shape.
const MERGE: f32 = 0.5;

impl ChainShape {
    /// The orthogonal convex hull of `rects`. Rectangles with no area are
    /// ignored; with none left the shape is empty.
    pub fn wrap(rects: &[Span]) -> Self {
        let rects: Vec<Span> = rects.iter().copied().filter(|r| r.2 - r.0 > MERGE && r.3 - r.1 > MERGE).collect();
        let edges = |pick: &dyn Fn(&Span) -> [f32; 2]| {
            let mut v: Vec<f32> = rects.iter().flat_map(pick).collect();
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < MERGE);
            v
        };
        let xs = edges(&|r| [r.0, r.2]);
        let ys = edges(&|r| [r.1, r.3]);
        let (nx, ny) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
        let mut inside = vec![false; nx * ny];
        // A cell is in a rectangle when its middle is.
        for r in &rects {
            for j in 0..ny {
                let my = (ys[j] + ys[j + 1]) * 0.5;
                if my < r.1 || my > r.3 {
                    continue;
                }
                for i in 0..nx {
                    let mx = (xs[i] + xs[i + 1]) * 0.5;
                    if mx >= r.0 && mx <= r.2 {
                        inside[j * nx + i] = true;
                    }
                }
            }
        }
        // Fill every row and every column between its first and last cell
        // until nothing changes: the orthogonally convex closure.
        loop {
            let mut changed = false;
            for j in 0..ny {
                let row = &mut inside[j * nx..(j + 1) * nx];
                if let (Some(a), Some(b)) = (row.iter().position(|&c| c), row.iter().rposition(|&c| c)) {
                    for c in &mut row[a..=b] {
                        changed |= !*c;
                        *c = true;
                    }
                }
            }
            for i in 0..nx {
                let first = (0..ny).find(|&j| inside[j * nx + i]);
                let last = (0..ny).rev().find(|&j| inside[j * nx + i]);
                if let (Some(a), Some(b)) = (first, last) {
                    for j in a..=b {
                        changed |= !inside[j * nx + i];
                        inside[j * nx + i] = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        Self { xs, ys, inside }
    }

    fn nx(&self) -> usize {
        self.xs.len().saturating_sub(1)
    }


    /// Whether (x, y) is in the shape.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let nx = self.nx();
        let cell = |v: &[f32], p: f32| {
            if v.len() < 2 || p < v[0] || p >= v[v.len() - 1] {
                return None;
            }
            Some(v.partition_point(|&e| e <= p) - 1)
        };
        match (cell(&self.xs, x), cell(&self.ys, y)) {
            (Some(i), Some(j)) => self.inside[j * nx + i],
            _ => false,
        }
    }

    /// The rectangle round it, `(x0, y0, x1, y1)`.
    pub fn bounds(&self) -> Span {
        match (self.xs.first(), self.xs.last(), self.ys.first(), self.ys.last()) {
            (Some(&x0), Some(&x1), Some(&y0), Some(&y1)) => (x0, y0, x1, y1),
            _ => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The area it covers, which is what tells the smaller of two
    /// overlapping shapes.
    pub fn area(&self) -> f32 {
        self.bands().iter().map(|&(y0, y1, l, r)| (y1 - y0) * (r - l)).sum()
    }

    /// Each row of cells the shape crosses, top to bottom, as
    /// `(y0, y1, left, right)`: being orthogonally convex, a row is one run.
    /// Consecutive rows with the same run are one.
    fn bands(&self) -> Vec<(f32, f32, f32, f32)> {
        let nx = self.nx();
        let mut out: Vec<(f32, f32, f32, f32)> = Vec::new();
        for j in 0..self.ys.len().saturating_sub(1) {
            let row = &self.inside[j * nx..(j + 1) * nx];
            let (Some(a), Some(b)) = (row.iter().position(|&c| c), row.iter().rposition(|&c| c)) else { continue };
            let (l, r) = (self.xs[a], self.xs[b + 1]);
            match out.last_mut() {
                Some(last) if last.1 == self.ys[j] && last.2 == l && last.3 == r => last.1 = self.ys[j + 1],
                _ => out.push((self.ys[j], self.ys[j + 1], l, r)),
            }
        }
        out
    }

    /// The outline, clockwise on screen (y down): one loop per run of
    /// touching bands, which for the hull of a connected set is one loop.
    /// Down the right side, along the bottom, up the left side.
    pub fn outline(&self) -> Vec<Vec<(f32, f32)>> {
        let mut loops = Vec::new();
        let bands = self.bands();
        let mut start = 0;
        while start < bands.len() {
            let mut end = start + 1;
            while end < bands.len() && bands[end].0 == bands[end - 1].1 {
                end += 1;
            }
            let run = &bands[start..end];
            let mut pts: Vec<(f32, f32)> = vec![(run[0].2, run[0].0)];
            for b in run {
                pts.push((b.3, b.0));
                pts.push((b.3, b.1));
            }
            for b in run.iter().rev() {
                pts.push((b.2, b.1));
                pts.push((b.2, b.0));
            }
            loops.push(simplify(pts));
            start = end;
        }
        loops
    }

    /// The horizontal strips the shape is made of, each with which of its
    /// corners — top left, top right, bottom right, bottom left — is a
    /// corner of the outline that turns OUT (convex), for a fill that rounds
    /// those.
    pub fn strips(&self) -> Vec<(Span, (bool, bool, bool, bool))> {
        let bands = self.bands();
        (0..bands.len())
            .map(|k| {
                let (y0, y1, l, r) = bands[k];
                let above = (k > 0 && bands[k - 1].1 == y0).then(|| bands[k - 1]);
                let below = (k + 1 < bands.len() && bands[k + 1].0 == y1).then(|| bands[k + 1]);
                let corners = (
                    above.is_none_or(|a| a.2 > l),
                    above.is_none_or(|a| a.3 < r),
                    below.is_none_or(|b| b.3 < r),
                    below.is_none_or(|b| b.2 > l),
                );
                ((l, y0, r, y1), corners)
            })
            .collect()
    }
}

/// Drop the points of a closed loop that are not corners: repeats, and
/// points on a straight run.
fn simplify(pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        if v.last() != Some(&p) {
            v.push(p);
        }
    }
    while v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    loop {
        let n = v.len();
        if n < 3 {
            return v;
        }
        let straight = (0..n).find(|&k| {
            let (a, b, c) = (v[(k + n - 1) % n], v[k], v[(k + 1) % n]);
            (a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1)
        });
        match straight {
            Some(k) => {
                v.remove(k);
            }
            None => return v,
        }
    }
}

/// One corner of an outline drawn rounded: the arc's centre, the angles it
/// runs between (start below end) and its radius, and where the straight
/// edges each side of it stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corner {
    pub center: (f32, f32),
    pub radius: f32,
    pub start: f32,
    pub end: f32,
    /// Where the edge coming in stops and the edge going out starts.
    pub from: (f32, f32),
    pub to: (f32, f32),
    /// The outline's own corner, which the arc cuts.
    pub at: (f32, f32),
    /// Whether the corner turns OUT; one that turns in is a fillet, its
    /// centre outside the shape.
    pub convex: bool,
}

/// A shape drawn with rounded corners: the fill as strips, each with the
/// radius of its four corners (top left, top right, bottom right, bottom
/// left; 0 where the corner is not the outline's or turns in), and the
/// outline as loops of corners. The two agree corner for corner, so the
/// fill never shows past the line.
#[derive(Debug, Clone, PartialEq)]
pub struct Rounded {
    pub strips: Vec<(Span, [f32; 4])>,
    pub loops: Vec<Vec<Corner>>,
}

impl ChainShape {
    /// The shape with its corners rounded at `radius`, each cut down to
    /// half of the shorter edge beside it, so two corners on a short step
    /// meet rather than cross, and a corner that turns out to half the
    /// strip it rounds, so the strip can hold it. A corner that turns IN
    /// is a fillet: its arc's centre is outside the shape.
    pub fn rounded(&self, radius: f32) -> Rounded {
        let strips = self.strips();
        let key = |p: (f32, f32)| (p.0.to_bits(), p.1.to_bits());
        // The cap each convex corner's strip puts on its radius.
        let mut cap: std::collections::HashMap<(u32, u32), f32> = std::collections::HashMap::new();
        for ((x0, y0, x1, y1), c) in &strips {
            let half = ((x1 - x0).min(y1 - y0)) * 0.5;
            for (on, p) in [(c.0, (*x0, *y0)), (c.1, (*x1, *y0)), (c.2, (*x1, *y1)), (c.3, (*x0, *y1))] {
                if on {
                    let e = cap.entry(key(p)).or_insert(half);
                    *e = e.min(half);
                }
            }
        }
        let len = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut radius_at: std::collections::HashMap<(u32, u32), f32> = std::collections::HashMap::new();
        let loops: Vec<Vec<Corner>> = self
            .outline()
            .iter()
            .map(|pts| {
                let n = pts.len();
                let radii: Vec<f32> = (0..n)
                    .map(|k| {
                        let (p, v, q) = (pts[(k + n - 1) % n], pts[k], pts[(k + 1) % n]);
                        let r = radius.min(len(p, v) * 0.5).min(len(v, q) * 0.5);
                        let r = cap.get(&key(v)).map_or(r, |c| r.min(*c)).max(0.0);
                        radius_at.insert(key(v), r);
                        r
                    })
                    .collect();
                round_corners(pts, &radii)
            })
            .collect();
        let strips = strips
            .into_iter()
            .map(|((x0, y0, x1, y1), c)| {
                let at = |on: bool, p: (f32, f32)| if on { radius_at.get(&key(p)).copied().unwrap_or(0.0) } else { 0.0 };
                ((x0, y0, x1, y1), [at(c.0, (x0, y0)), at(c.1, (x1, y0)), at(c.2, (x1, y1)), at(c.3, (x0, y1))])
            })
            .collect();
        Rounded { strips, loops }
    }
}

/// The corners of a loop, each rounded at its own radius.
pub fn round_corners(pts: &[(f32, f32)], radii: &[f32]) -> Vec<Corner> {
    let n = pts.len();
    let len = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    (0..n)
        .map(|k| {
            let (p, v, q) = (pts[(k + n - 1) % n], pts[k], pts[(k + 1) % n]);
            let (li, lo) = (len(p, v), len(v, q));
            let r = radii[k];
            let din = if li > 0.0 { ((v.0 - p.0) / li, (v.1 - p.1) / li) } else { (0.0, 0.0) };
            let dout = if lo > 0.0 { ((q.0 - v.0) / lo, (q.1 - v.1) / lo) } else { (0.0, 0.0) };
            let from = (v.0 - din.0 * r, v.1 - din.1 * r);
            let to = (v.0 + dout.0 * r, v.1 + dout.1 * r);
            let center = (from.0 + dout.0 * r, from.1 + dout.1 * r);
            let a1 = (from.1 - center.1).atan2(from.0 - center.0);
            let mut d = (to.1 - center.1).atan2(to.0 - center.0) - a1;
            while d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            while d <= -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            let (start, end) = if d >= 0.0 { (a1, a1 + d) } else { (a1 + d, a1) };
            // Clockwise on screen (y down), a right turn is a turn out.
            let convex = din.0 * dout.1 - din.1 * dout.0 > 0.0;
            Corner { center, radius: r, start, end, from, to, at: v, convex }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_l_of_rects_is_an_l_and_not_its_bounding_box() {
        // A column, then a row off its foot.
        let s = ChainShape::wrap(&[(0.0, 0.0, 10.0, 30.0), (0.0, 20.0, 40.0, 30.0)]);
        assert!(s.contains(5.0, 5.0) && s.contains(35.0, 25.0));
        assert!(!s.contains(35.0, 5.0), "the empty corner is not in it");
        assert_eq!(s.area(), 10.0 * 30.0 + 30.0 * 10.0);
        assert_eq!(s.outline(), vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 20.0), (40.0, 20.0), (40.0, 30.0), (0.0, 30.0)]]);
    }

    #[test]
    fn a_gap_between_two_rects_one_above_the_other_is_filled() {
        // Rows of nodes stand a little apart; the column fill joins them.
        let s = ChainShape::wrap(&[(0.0, 0.0, 20.0, 10.0), (5.0, 12.0, 30.0, 22.0)]);
        assert!(s.contains(10.0, 11.0), "the gap where they overlap in x");
        assert!(!s.contains(2.0, 11.0) && !s.contains(25.0, 11.0), "but not beside it");
        assert_eq!(s.outline().len(), 1, "one piece");
    }

    #[test]
    fn a_staircase_has_convex_and_concave_corners() {
        let s = ChainShape::wrap(&[(0.0, 0.0, 10.0, 10.0), (0.0, 10.0, 20.0, 20.0)]);
        let strips = s.strips();
        assert_eq!(strips.len(), 2);
        assert_eq!(strips[0].1, (true, true, false, false));
        assert_eq!(strips[1].1, (false, true, true, true));
        let loop0 = &s.outline()[0];
        let rounded = s.rounded(4.0);
        let corners = &rounded.loops[0];
        // The step at (10, 10) turns in: its centre is outside the shape.
        let k = loop0.iter().position(|&p| p == (10.0, 10.0)).unwrap();
        assert!(!s.contains(corners[k].center.0, corners[k].center.1) && !corners[k].convex);
        let k = loop0.iter().position(|&p| p == (20.0, 10.0)).unwrap();
        assert!(s.contains(corners[k].center.0, corners[k].center.1) && corners[k].convex);
        assert!(corners.iter().all(|c| (c.end - c.start - std::f32::consts::FRAC_PI_2).abs() < 1e-4));
        // The fill rounds the convex corners at the outline's radii and no
        // others.
        assert_eq!(rounded.strips[0].1, [4.0, 4.0, 0.0, 0.0]);
        assert_eq!(rounded.strips[1].1, [0.0, 4.0, 4.0, 4.0]);
        // A short step cuts its corners down.
        let thin = ChainShape::wrap(&[(0.0, 0.0, 40.0, 40.0), (0.0, 40.0, 42.0, 80.0)]).rounded(10.0);
        assert!(thin.loops[0].iter().all(|c| c.radius <= 1.0 + 1e-4 || c.radius == 10.0));
    }
}
