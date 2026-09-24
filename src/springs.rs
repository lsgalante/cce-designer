//! The edge-length spring solve behind Relax's Springs mode, on the CPU and
//! on the GPU — the first operator of Phase 7 step 4, and the pattern for
//! the ones that follow: one algorithm, one data layout, two backends held
//! to each other by a cross-check.
//!
//! It is a JACOBI solve. Every point gathers the corrections of all its
//! incident edges from the positions as they stood at the start of the
//! pass, averages them, and moves once; the pass is one dispatch, one
//! invocation per point. Until 2026-09-24 the CPU solver was Gauss–Seidel
//! over the edge list in sequence, each correction visible to the next
//! edge, which no per-point kernel can reproduce — so the parallel form is
//! the one both backends share, and it is what the CPU runs too, rather
//! than letting the two drift apart. Jacobi converges more slowly per
//! iteration (roughly half the rate), which Iterations already controls.
//!
//! The layout is the GPU's: positions as a flat `xyz` float array (a
//! `vec3<f32>` in a WGSL storage array is padded to 16 bytes), the rest
//! topology as CSR — `offsets[p]..offsets[p + 1]` index the incident edges
//! of point `p`, each with its neighbour and its rest length — and pins as
//! one `u32` per point. The CPU solver walks exactly the same arrays in
//! exactly the same order, so the two agree to floating-point noise.

use crate::detail::Detail;
use cce_ui::vk::{Binding, ComputeDevice, Kernel};
use glam::Vec3;

/// The rest topology in CSR form, plus the pins, ready for either backend.
pub struct SpringSystem {
    pub n: usize,
    pub offsets: Vec<u32>,
    pub neighbour: Vec<u32>,
    pub rest: Vec<f32>,
    pub pinned: Vec<u32>,
}

impl SpringSystem {
    /// From the REST geometry's unique edges and a per-point pin flag.
    pub fn build(rest: &Detail, pinned: &[bool]) -> Self {
        let n = rest.num_points();
        let mut counts = vec![0u32; n];
        for e in rest.edges() {
            counts[e[0] as usize] += 1;
            counts[e[1] as usize] += 1;
        }
        let mut offsets = vec![0u32; n + 1];
        for p in 0..n {
            offsets[p + 1] = offsets[p] + counts[p];
        }
        let total = offsets[n] as usize;
        let mut neighbour = vec![0u32; total];
        let mut rest_len = vec![0f32; total];
        let mut fill = offsets[..n].to_vec();
        for e in rest.edges() {
            let (a, b) = (e[0] as usize, e[1] as usize);
            let len = (rest.pos(b) - rest.pos(a)).length();
            let ia = fill[a] as usize;
            neighbour[ia] = b as u32;
            rest_len[ia] = len;
            fill[a] += 1;
            let ib = fill[b] as usize;
            neighbour[ib] = a as u32;
            rest_len[ib] = len;
            fill[b] += 1;
        }
        SpringSystem {
            n,
            offsets,
            neighbour,
            rest: rest_len,
            pinned: (0..n).map(|p| u32::from(pinned.get(p).copied().unwrap_or(false))).collect(),
        }
    }

    pub fn num_edges(&self) -> usize {
        self.neighbour.len() / 2
    }
}

/// One Jacobi pass on the CPU: `out` from `pos`.
fn pass_cpu(sys: &SpringSystem, stiffness: f32, pos: &[f32], out: &mut [f32]) {
    for p in 0..sys.n {
        let base = p * 3;
        let x = Vec3::new(pos[base], pos[base + 1], pos[base + 2]);
        let mut x_new = x;
        if sys.pinned[p] == 0 {
            let (start, end) = (sys.offsets[p] as usize, sys.offsets[p + 1] as usize);
            let mut sum = Vec3::ZERO;
            for e in start..end {
                let q = sys.neighbour[e] as usize;
                let y = Vec3::new(pos[q * 3], pos[q * 3 + 1], pos[q * 3 + 2]);
                let d = y - x;
                let len = d.length();
                if len < 1e-6 {
                    continue;
                }
                // Half the error toward a free neighbour (it moves the other
                // half); all of it toward a pinned one, which does not move.
                let w = if sys.pinned[q] != 0 { 1.0 } else { 0.5 };
                sum += d * ((len - sys.rest[e]) / len * w * stiffness);
            }
            let count = end - start;
            if count > 0 {
                x_new = x + sum / count as f32;
            }
        }
        out[base] = x_new.x;
        out[base + 1] = x_new.y;
        out[base + 2] = x_new.z;
    }
}

/// `iterations` Jacobi passes on the CPU, in place.
pub fn solve_cpu(sys: &SpringSystem, stiffness: f32, iterations: usize, pos: &mut Vec<f32>) {
    let mut out = vec![0f32; pos.len()];
    for _ in 0..iterations {
        pass_cpu(sys, stiffness, pos, &mut out);
        std::mem::swap(pos, &mut out);
    }
}

/// The same pass as a WGSL kernel: one invocation per point.
pub const SPRINGS_WGSL: &str = r#"
struct Params { stiffness: f32, n: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<storage, read> pos_in: array<f32>;
@group(0) @binding(1) var<storage, read_write> pos_out: array<f32>;
@group(0) @binding(2) var<storage, read> offsets: array<u32>;
@group(0) @binding(3) var<storage, read> neighbour: array<u32>;
@group(0) @binding(4) var<storage, read> rest: array<f32>;
@group(0) @binding(5) var<storage, read> pinned: array<u32>;
@group(0) @binding(6) var<uniform> params: Params;

@compute @workgroup_size(64)
fn springs(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.x;
    if (p >= params.n) { return; }
    let base = p * 3u;
    let x = vec3<f32>(pos_in[base], pos_in[base + 1u], pos_in[base + 2u]);
    var x_new = x;
    if (pinned[p] == 0u) {
        let start = offsets[p];
        let end = offsets[p + 1u];
        var sum = vec3<f32>(0.0, 0.0, 0.0);
        for (var e = start; e < end; e = e + 1u) {
            let q = neighbour[e];
            let y = vec3<f32>(pos_in[q * 3u], pos_in[q * 3u + 1u], pos_in[q * 3u + 2u]);
            let d = y - x;
            let len = length(d);
            if (len < 1e-6) { continue; }
            var w = 0.5;
            if (pinned[q] != 0u) { w = 1.0; }
            sum = sum + d * ((len - rest[e]) / len * w * params.stiffness);
        }
        let count = end - start;
        if (count > 0u) { x_new = x + sum / f32(count); }
    }
    pos_out[base] = x_new.x;
    pos_out[base + 1u] = x_new.y;
    pos_out[base + 2u] = x_new.z;
}"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    stiffness: f32,
    n: u32,
    pad0: u32,
    pad1: u32,
}

/// `iterations` Jacobi passes on the GPU, in place: ONE submission, the
/// passes chained by memory barriers and the positions ping-ponging between
/// two device buffers, so the topology goes up once and the result comes
/// back once. The first cut submitted a pass at a time — upload, dispatch,
/// wait, read back, sixteen times — and lost to the CPU at every mesh size
/// measured, 134k points included: a submission's round trip is about half
/// a millisecond on an integrated GPU whatever the dispatch inside it, and
/// the solve itself is far cheaper than that. `springs_timing` has the
/// numbers for both shapes.
pub fn solve_gpu(
    dev: &mut ComputeDevice,
    sys: &SpringSystem,
    stiffness: f32,
    iterations: usize,
    pos: &mut Vec<f32>,
) -> Result<(), String> {
    if sys.n == 0 || iterations == 0 {
        return Ok(());
    }
    let kernel = Kernel::new(SPRINGS_WGSL, "springs");
    let params = Params { stiffness, n: sys.n as u32, pad0: 0, pad1: 0 };
    let mut out = vec![0f32; pos.len()];
    dev.run_passes_over(
        &kernel,
        &mut [
            Binding::input(pos.as_slice()),
            Binding::rw(out.as_mut_slice()),
            Binding::input(&sys.offsets),
            Binding::input(&sys.neighbour),
            Binding::input(&sys.rest),
            Binding::input(&sys.pinned),
            Binding::uniform(&params),
        ],
        sys.n as u32,
        iterations as u32,
        Some((0, 1)),
    )?;
    *pos = out;
    Ok(())
}

/// Below this many points the CPU is faster; in auto mode it takes them.
/// From `springs_timing` in release on an Intel Iris Xe, sixteen passes,
/// one submission: 1.5k points cpu 0.25 ms / gpu 1.5 ms; 15k points cpu
/// 2.6 ms / gpu 3.7 ms; 135k points cpu 25 ms / gpu 14 ms. Break-even is
/// in the tens of thousands, and a first run adds ~15 ms of pipeline
/// compile on top; 32k is where the GPU is clearly ahead on every run
/// after the first.
pub const GPU_MIN_POINTS: usize = 32_768;

/// The solve as Relax runs it: the backend `CCE_COMPUTE` and the size
/// choose, a GPU failure in auto mode falling back to the CPU with a note,
/// and in forced-GPU mode reported back for the node-error slot.
pub fn solve(sys: &SpringSystem, stiffness: f32, iterations: usize, pos: &mut Vec<f32>) -> Result<(), String> {
    if crate::gpu::use_gpu(sys.n, GPU_MIN_POINTS) {
        let attempt = crate::gpu::with_any_device(|dev| solve_gpu(dev, sys, stiffness, iterations, pos));
        match attempt {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) | Err(e) => {
                if crate::gpu::choice() == crate::gpu::Choice::Gpu {
                    solve_cpu(sys, stiffness, iterations, pos);
                    return Err(format!("CCE_COMPUTE=gpu but the springs solve could not run there ({e}); solved on the CPU"));
                }
                note_fallback_once(&e);
            }
        }
    }
    solve_cpu(sys, stiffness, iterations, pos);
    Ok(())
}

fn note_fallback_once(e: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("cce-designer: springs solve fell back to the CPU: {e}"));
}
