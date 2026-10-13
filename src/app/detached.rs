//! `State::detached`: detached windows. A pane can be handed to a window
//! of its own — another process of this binary, spawned with a
//! `--detached-*` flag — and the two keep in step through one sync channel,
//! `default_project.json`, autosaved by whichever changed it and polled by
//! the other (`poll_shared_project`). This is that: whether this process is
//! such a window and for which pane, the panes and child processes the
//! main window handed out, and the channel's write and poll timing.

use super::WIDGET_COUNT;

pub struct DetachedWindows {
    /// This process IS the detached network window (`--detached-network`):
    /// circular, the network pane alone.
    pub is_detached_network: bool,
    /// The MAIN window has handed its network to a detached circular window
    /// (`detach_circular_window`).
    pub detached_circular_network: bool,
    /// This process IS the detached window for one pane — the generic sibling
    /// of `is_detached_network`, which stays its own flag because the network's
    /// detached window is not merely detached: it is CIRCULAR, with a radial
    /// border resize and custom CSD that no rectangular pane wants.
    pub detached_pane: Option<usize>,
    /// The MAIN window's record of which panes it has handed to a detached
    /// window, so it lays them out as stubs. `detached_circular_network` is the
    /// network's equivalent.
    pub detached_panes: [bool; WIDGET_COUNT],
    /// The detached child PROCESS per pane — so Reattach can close the window it
    /// is taking the pane back from, and so the parent can notice a child the
    /// user closed themselves and take the pane back on its own.
    ///
    /// The handle, not a bare pid: an exited child the parent never waits on is
    /// a ZOMBIE, and `kill(pid, 0)` succeeds for zombies — a pid-based liveness
    /// probe reports a closed window as still running, forever. `try_wait`
    /// reaps and reports for real.
    pub detached_children: std::collections::HashMap<usize, std::process::Child>,
    /// The sync channel's mtime as this process last wrote or read it: a
    /// poll that finds another is the other window's write, and reloads.
    pub last_project_mod_time: Option<std::time::SystemTime>,
    /// When the channel was last polled (every 100 ms at most).
    pub last_project_check: std::time::Instant,
    /// Something the other window shows changed here: write the channel,
    /// at most every 200 ms (`last_autosave_time`), and at exit.
    pub needs_autosave: bool,
    pub last_autosave_time: std::time::Instant,
}
