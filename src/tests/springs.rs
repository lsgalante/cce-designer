//! The springs solve (src/springs.rs): CPU and GPU, one algorithm.

use super::*;

/// A rest sphere, its points stretched and stirred, a few pinned: the
/// fixture both backends solve.
fn springs_fixture(rows: usize, cols: usize) -> (crate::springs::SpringSystem, Vec<f32>, Vec<bool>) {
    let rest = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, rows, cols);
    let n = rest.num_points();
    let pinned: Vec<bool> = (0..n).map(|p| p % 97 == 0).collect();
    let sys = crate::springs::SpringSystem::build(&rest, &pinned);
    let pos: Vec<f32> = (0..n)
        .flat_map(|p| {
            let x = rest.pos(p);
            let stretched = x * 1.4 + Vec3::new((p as f32 * 0.37).sin() * 0.05, 0.0, (p as f32 * 0.11).cos() * 0.05);
            [stretched.x, stretched.y, stretched.z]
        })
        .collect();
    (sys, pos, pinned)
}

/// The CSR is the rest topology twice over — every edge once from each
/// end — with the rest length on both entries.
#[test]
fn springs_system_is_the_rest_topology_in_csr_form() {
    let rest = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, 6, 8);
    let sys = crate::springs::SpringSystem::build(&rest, &[]);
    assert_eq!(sys.n, rest.num_points());
    assert_eq!(sys.num_edges(), rest.edges().len());
    assert_eq!(sys.neighbour.len(), 2 * rest.edges().len());
    for p in 0..sys.n {
        let (s, e) = (sys.offsets[p] as usize, sys.offsets[p + 1] as usize);
        assert_eq!(e - s, rest.point_neighbours(p).len(), "point {p}'s incident count is its valence");
        for i in s..e {
            let q = sys.neighbour[i] as usize;
            assert!((sys.rest[i] - (rest.pos(q) - rest.pos(p)).length()).abs() < 1e-6);
        }
    }
    assert!(sys.pinned.iter().all(|&v| v == 0), "no pins asked for, none set");
}

/// Jacobi restores the rest lengths and never moves a pin.
#[test]
fn springs_cpu_restores_rest_lengths_and_holds_pins() {
    let (sys, mut pos, pinned) = springs_fixture(12, 16);
    let before = pos.clone();
    let error = |pos: &[f32]| -> f32 {
        let mut worst: f32 = 0.0;
        for p in 0..sys.n {
            for e in sys.offsets[p] as usize..sys.offsets[p + 1] as usize {
                let q = sys.neighbour[e] as usize;
                let d = Vec3::new(pos[q * 3] - pos[p * 3], pos[q * 3 + 1] - pos[p * 3 + 1], pos[q * 3 + 2] - pos[p * 3 + 2]);
                worst = worst.max((d.length() - sys.rest[e]).abs() / sys.rest[e]);
            }
        }
        worst
    };
    let start = error(&pos);
    assert!(start > 0.3, "the fixture is stretched: {start}");
    crate::springs::solve_cpu(&sys, 1.0, 60, &mut pos);
    let after = error(&pos);
    assert!(after < start * 0.25, "sixty passes at full stiffness pull the edges toward rest: {start} -> {after}");
    for p in 0..sys.n {
        if pinned[p] {
            assert_eq!(&pos[p * 3..p * 3 + 3], &before[p * 3..p * 3 + 3], "pin {p} moved");
        }
    }
    // Zero stiffness or zero iterations: nothing moves.
    let mut still = before.clone();
    crate::springs::solve_cpu(&sys, 0.0, 10, &mut still);
    assert_eq!(still, before);
    crate::springs::solve_cpu(&sys, 1.0, 0, &mut still);
    assert_eq!(still, before);
}

/// The cross-check that holds the GPU to the CPU: the same passes over
/// the same arrays agree to floating-point noise. Skips, with a note,
/// where the machine has no Vulkan.
#[test]
fn springs_gpu_matches_cpu() {
    // Well under the auto threshold: the cross-check asks for the GPU by
    // name, and a small mesh keeps it quick.
    let (sys, pos0, _) = springs_fixture(32, 48);
    let mut cpu = pos0.clone();
    crate::springs::solve_cpu(&sys, 0.7, 12, &mut cpu);
    let mut gpu = pos0.clone();
    let ran = crate::gpu::with_any_device(|dev| crate::springs::solve_gpu(dev, &sys, 0.7, 12, &mut gpu));
    match ran {
        Err(e) => {
            println!("skipping springs_gpu_matches_cpu: {e}");
            return;
        }
        Ok(r) => r.expect("the springs kernel runs"),
    }
    let mut worst: f32 = 0.0;
    for (i, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
        let d = (a - b).abs();
        assert!(d < 1e-4, "component {i}: cpu {a} gpu {b}");
        worst = worst.max(d);
    }
    assert!(cpu != pos0, "the solve did something");
    println!("springs gpu vs cpu: worst component difference {worst:e} over {} points", sys.n);
}

/// `CCE_COMPUTE` parses as written, and under test auto means CPU so the
/// suite is the same on every machine.
#[test]
fn compute_choice_parses_the_variable_and_defaults_to_cpu_under_test() {
    use crate::gpu::{parse, Choice};
    assert_eq!(parse(None), Choice::Cpu, "auto is CPU under cfg(test)");
    assert_eq!(parse(Some("auto")), Choice::Cpu);
    assert_eq!(parse(Some("gpu")), Choice::Gpu);
    assert_eq!(parse(Some(" CPU ")), Choice::Cpu, "case-insensitive, trimmed");
    assert_eq!(parse(Some("banana")), Choice::Cpu, "nonsense is auto, with a note");
    // The suite never sets the variable: a test that did would race every
    // other test reading it, and the GPU is exercised by name instead.
    assert!(std::env::var_os("CCE_COMPUTE").is_none());
}

/// Where the auto threshold sits, and a rough measure of why: the CPU
/// and GPU solve timed over a large sphere. Ignored — it is a
/// measurement, not an assertion — run with
/// `cargo test --release -p cce-designer springs_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn springs_timing() {
    for (rows, cols) in [(16, 24), (32, 48), (100, 150), (300, 450)] {
        let (sys, pos0, _) = springs_fixture(rows, cols);
        let t = std::time::Instant::now();
        let mut cpu = pos0.clone();
        crate::springs::solve_cpu(&sys, 0.7, 16, &mut cpu);
        let cpu_ms = t.elapsed().as_secs_f64() * 1e3;
        let mut gpu = pos0.clone();
        let t = std::time::Instant::now();
        let ran = crate::gpu::with_any_device(|dev| crate::springs::solve_gpu(dev, &sys, 0.7, 16, &mut gpu));
        let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
        println!("{:>7} points, 16 passes: cpu {cpu_ms:8.2} ms   gpu {gpu_ms:8.2} ms   ({:?})", sys.n, ran.map(|r| r.is_ok()));
    }
}
