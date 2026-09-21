use cce_ui::widget::{Key, NamedKey};
use crate::ModifiersState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    ToggleGrid,
    ToggleCube,
    ToggleSquareViewport,
    ToggleConfigure,
    ToggleSpreadsheet,
    ToggleOrigin,
    ToggleCameraPivot,
    ToggleWireframe,
    WireframeColor,
    ToggleCircularPane,
    DetachCircularWindow,
    Save,
    SaveAs,
    NextContext,
    PrevContext,
    PlayPause,
    PlayPauseReverse,
    FrameNext,
    FramePrev,
    Undo,
    Redo,
    /// Open the command palette — a command like any other, so it is
    /// rebindable and lists itself.
    CommandPalette,
    /// Open or close the in-app dialog (`src/dialog.rs`): the same commands,
    /// plus the viewport/graph settings, without leaving the window.
    ToggleDialog,
    /// Snap dragged handles in the active viewer state to a world increment.
    ToggleSnap,
    /// Enter or leave the selected node's viewer state.
    ToggleViewerState,
    /// Network-pane keyboard navigation, in the plugin's vim-style families:
    /// bare hjkl moves the grid cursor, alt moves the node under it, ctrl pans
    /// the view. The direction rides the variant so one registry row binds one
    /// key, which is what a rebindable scheme needs.
    NetworkNav(i32, i32),
    NetworkMove(i32, i32),
    NetworkPan(i32, i32),
    FrameCursor,
    FrameAll,
    /// Arrange the current level's nodes from their wiring.
    LayoutNodes,
    /// Draw the network pane's plate, or let the graph overlay the scene.
    ToggleNetworkPlate,
    /// Clear the node selection.
    Deselect,
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    pub key: Key,
}

impl Shortcut {
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut logo = false;
        let mut key_opt = None;

        if parts.is_empty() {
            return Err("Empty shortcut".to_string());
        }

        for (i, &part) in parts.iter().enumerate() {
            let lower = part.to_lowercase();
            if i < parts.len() - 1 {
                match lower.as_str() {
                    "ctrl" | "control" => ctrl = true,
                    "shift" => shift = true,
                    "alt" => alt = true,
                    "super" | "win" | "logo" | "cmd" | "command" => logo = true,
                    _ => return Err(format!("Unknown modifier: {}", part)),
                }
            } else {
                let key = match lower.as_str() {
                    "tab" => Key::Named(NamedKey::Tab),
                    "enter" | "return" => Key::Named(NamedKey::Enter),
                    "escape" | "esc" => Key::Named(NamedKey::Escape),
                    "space" => Key::Named(NamedKey::Space),
                    "backspace" => Key::Named(NamedKey::Backspace),
                    "down" | "arrowdown" => Key::Named(NamedKey::ArrowDown),
                    "up" | "arrowup" => Key::Named(NamedKey::ArrowUp),
                    "left" | "arrowleft" => Key::Named(NamedKey::ArrowLeft),
                    "right" | "arrowright" => Key::Named(NamedKey::ArrowRight),
                    "end" => Key::Named(NamedKey::End),
                    "home" => Key::Named(NamedKey::Home),
                    "pagedown" | "pgdown" => Key::Named(NamedKey::PageDown),
                    "pageup" | "pgup" => Key::Named(NamedKey::PageUp),
                    "delete" | "del" => Key::Named(NamedKey::Delete),
                    _ => Key::Character(part.to_string()),
                };
                key_opt = Some(key);
            }
        }

        let key = key_opt.ok_or_else(|| "Missing key in shortcut".to_string())?;
        Ok(Shortcut { ctrl, shift, alt, logo, key })
    }

    /// The chord as a human reads it — the inverse of [`parse`](Self::parse),
    /// for showing beside a command's label.
    ///
    /// Modifier order is fixed (Ctrl, Shift, Alt, Super) rather than however
    /// the user happened to write it, so two spellings of one chord print the
    /// same and a palette column stays scannable.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for (on, name) in
            [(self.ctrl, "Ctrl"), (self.shift, "Shift"), (self.alt, "Alt"), (self.logo, "Super")]
        {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        match &self.key {
            Key::Character(c) => {
                // Single letters read as capitals — "Ctrl+S", not "Ctrl+s" —
                // which is how every menu in the app already writes them.
                if c.chars().count() == 1 {
                    out.extend(c.chars().flat_map(|ch| ch.to_uppercase()));
                } else {
                    out.push_str(c);
                }
            }
            Key::Named(n) => out.push_str(&format!("{n:?}")),
        }
        out
    }

    pub fn matches(&self, mods: &ModifiersState, key: &Key) -> bool {
        if mods.control_key() != self.ctrl
            || mods.shift_key() != self.shift
            || mods.alt_key() != self.alt
            || mods.super_key() != self.logo
        {
            return false;
        }
        same_key(key, &self.key)
    }
}

/// Whether two keys are the same key.
///
/// Character keys compare case-insensitively: with Shift held, xkb delivers
/// the SHIFTED character ("S"), so an exact match against the chord's stored
/// "s" made every Shift+letter chord unmatchable — Ctrl+Shift+Tab never
/// noticed because Named keys aren't shifted.
fn same_key(a: &Key, b: &Key) -> bool {
    match (a, b) {
        (Key::Character(x), Key::Character(y)) => x.eq_ignore_ascii_case(y),
        (x, y) => x == y,
    }
}

/// Equality is what the KEYBOARD would call the same chord, which is why it is
/// written rather than derived.
///
/// The derived version compared character keys byte for byte while `matches`
/// compared them case-insensitively, so the two disagreed: `Ctrl+S` and
/// `Ctrl+s` are one keypress at the keyboard and were two distinct `Shortcut`s
/// in memory. Nothing noticed until `command::conflicts` started comparing
/// chords to each other and quietly failed to report a collision between two
/// spellings of the same binding — the exact failure it exists to catch. Both
/// go through `same_key` now, so they cannot drift again.
impl PartialEq for Shortcut {
    fn eq(&self, other: &Self) -> bool {
        self.ctrl == other.ctrl
            && self.shift == other.shift
            && self.alt == other.alt
            && self.logo == other.logo
            && same_key(&self.key, &other.key)
    }
}

impl Eq for Shortcut {}

/// Chord -> command id.
///
/// Ids rather than [`Action`]s because a chord has to be able to reach a
/// command the `Action` enum does not cover — New Project and Open are menu
/// labels, and there was no way to bind them at all while this held `Action`.
/// What a binding names is a row in [`crate::command::COMMANDS`], and that row
/// says how to run it.
pub struct ShortcutManager {
    bindings: Vec<(Shortcut, &'static str)>,
}

impl ShortcutManager {
    pub fn new() -> Self {
        ShortcutManager { bindings: Vec::new() }
    }

    pub fn register(&mut self, shortcut_str: &str, command: &'static str) -> Result<(), String> {
        let shortcut = Shortcut::parse(shortcut_str)?;
        self.bindings.push((shortcut, command));
        Ok(())
    }

    /// First match wins, in registration order — which is registry order. Two
    /// commands on one chord therefore make the second unreachable in silence,
    /// which is why `command::conflicts` exists to say so at startup.
    pub fn match_command(&self, mods: &ModifiersState, key: &Key) -> Option<&'static str> {
        for (shortcut, command) in &self.bindings {
            if shortcut.matches(mods, key) {
                return Some(command);
            }
        }
        None
    }

    /// The chord bound to `command`, for showing beside its label.
    pub fn chord_for(&self, command: &str) -> Option<&Shortcut> {
        self.bindings.iter().find(|(_, c)| *c == command).map(|(s, _)| s)
    }
}
