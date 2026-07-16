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
    ToggleCircularPane,
    DetachCircularWindow,
    Save,
    NextContext,
    PrevContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub fn matches(&self, mods: &ModifiersState, key: &Key) -> bool {
        mods.control_key() == self.ctrl
            && mods.shift_key() == self.shift
            && mods.alt_key() == self.alt
            && mods.super_key() == self.logo
            && key == &self.key
    }
}

pub struct ShortcutManager {
    bindings: Vec<(Shortcut, Action)>,
}

impl ShortcutManager {
    pub fn new() -> Self {
        ShortcutManager { bindings: Vec::new() }
    }

    pub fn register(&mut self, shortcut_str: &str, action: Action) -> Result<(), String> {
        let shortcut = Shortcut::parse(shortcut_str)?;
        self.bindings.push((shortcut, action));
        Ok(())
    }

    pub fn match_action(&self, mods: &ModifiersState, key: &Key) -> Option<Action> {
        for (shortcut, action) in &self.bindings {
            if shortcut.matches(mods, key) {
                return Some(*action);
            }
        }
        None
    }
}
