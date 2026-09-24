//! The evaluation thread's compute device, and the switch that says whether
//! an operator may use it. Phase 7 step 4 of `shapeshifter.md`.
//!
//! One [`ComputeDevice`] per thread, opened on first use and kept, so the
//! pipeline cache and the buffers survive from one edit to the next: a
//! device costs tens of milliseconds to open and a kernel a few to compile,
//! and an operator that paid both per evaluation would lose to the CPU
//! every time. Thread-local because the device is not `Send`, and because
//! the app evaluates on its main thread while the thumbnail and export
//! CLIs evaluate on theirs.
//!
//! `CCE_COMPUTE` decides: unset (or `auto`) uses the GPU when one is there
//! and the operator judges the input large enough to be worth the round
//! trip; `cpu` never opens a device; `gpu` insists, and an operator that
//! cannot get one reports it through the node-error slot rather than
//! silently taking the CPU path. Under `cfg(test)` auto means CPU, so the
//! suite is deterministic on every machine and the GPU is exercised only by
//! the tests that ask for it by name — the cross-checks that hold each GPU
//! operator to its CPU twin.

use cce_ui::vk::ComputeDevice;
use std::cell::RefCell;

/// What `CCE_COMPUTE` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Cpu,
    Gpu,
}

pub fn choice() -> Choice {
    parse(std::env::var("CCE_COMPUTE").ok().as_deref())
}

/// `CCE_COMPUTE`'s value to a choice — a pure function of the text, so the
/// tests can cover it without touching the process environment, which
/// libtest's parallel tests would race on.
pub fn parse(value: Option<&str>) -> Choice {
    let auto = if cfg!(test) { Choice::Cpu } else { Choice::Auto };
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("cpu") => Choice::Cpu,
        Some("gpu") => Choice::Gpu,
        Some("auto") | Some("") | None => auto,
        Some(other) => {
            eprintln!("cce-designer: CCE_COMPUTE={other:?} is not cpu, gpu or auto; using auto");
            auto
        }
    }
}

thread_local! {
    static DEVICE: RefCell<Option<Result<ComputeDevice, String>>> = const { RefCell::new(None) };
}

/// Run `f` on this thread's device, opening it on first call. `Err` when
/// the machine has no usable Vulkan — the same answer every call, since a
/// failed open is remembered rather than retried per evaluation.
pub fn with_any_device<R>(f: impl FnOnce(&mut ComputeDevice) -> R) -> Result<R, String> {
    DEVICE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let opened = ComputeDevice::new();
            match &opened {
                Ok(d) => eprintln!("cce-designer: compute on {}", d.device_name()),
                Err(e) => eprintln!("cce-designer: no compute device ({e}); operators run on the CPU"),
            }
            *slot = Some(opened);
        }
        match slot.as_mut().unwrap() {
            Ok(d) => Ok(f(d)),
            Err(e) => Err(e.clone()),
        }
    })
}

/// Whether an operator with `points` elements should take the GPU: the
/// choice, and in auto mode the operator's own threshold.
pub fn use_gpu(points: usize, auto_threshold: usize) -> bool {
    match choice() {
        Choice::Cpu => false,
        Choice::Gpu => true,
        Choice::Auto => points >= auto_threshold,
    }
}
