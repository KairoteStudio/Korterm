// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! User-configurable keyboard shortcuts.
//!
//! Each action maps to a combo string like `"ctrl+shift+c"`. Defaults live
//! here; overrides are persisted in the app config (`keybind_<id>` keys).

use iced::keyboard::{Key, Modifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Copy,
    Paste,
    Search,
    QuickToggle,
    NewTerm,
    CloseTerm,
    Clear,
    NextTab,
    PrevTab,
    Settings,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollHome,
    ScrollEnd,
}

impl Action {
    /// Stable id used in the config file.
    pub fn id(self) -> &'static str {
        match self {
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::Search => "search",
            Action::QuickToggle => "quick",
            Action::NewTerm => "new_term",
            Action::CloseTerm => "close_term",
            Action::Clear => "clear",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::Settings => "settings",
            Action::ScrollUp => "scroll_up",
            Action::ScrollDown => "scroll_down",
            Action::ScrollPageUp => "scroll_page_up",
            Action::ScrollPageDown => "scroll_page_down",
            Action::ScrollHome => "scroll_home",
            Action::ScrollEnd => "scroll_end",
        }
    }

    /// Chinese label for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Action::Copy => "复制",
            Action::Paste => "粘贴",
            Action::Search => "搜索终端",
            Action::QuickToggle => "快速终端",
            Action::NewTerm => "新建终端",
            Action::CloseTerm => "关闭当前终端",
            Action::Clear => "清除终端",
            Action::NextTab => "下一个标签",
            Action::PrevTab => "上一个标签",
            Action::Settings => "设置",
            Action::ScrollUp => "向上滚动",
            Action::ScrollDown => "向下滚动",
            Action::ScrollPageUp => "向上翻页",
            Action::ScrollPageDown => "向下翻页",
            Action::ScrollHome => "滚动到顶部",
            Action::ScrollEnd => "滚动到底部",
        }
    }

    pub const ALL: [Action; 16] = [
        Action::Copy,
        Action::Paste,
        Action::Search,
        Action::QuickToggle,
        Action::NewTerm,
        Action::CloseTerm,
        Action::Clear,
        Action::NextTab,
        Action::PrevTab,
        Action::Settings,
        Action::ScrollUp,
        Action::ScrollDown,
        Action::ScrollPageUp,
        Action::ScrollPageDown,
        Action::ScrollHome,
        Action::ScrollEnd,
    ];
}

/// `(action, combo)` pairs, in display order.
pub type Bindings = Vec<(Action, String)>;

// The keyboard listener must be a capture-free `fn` pointer (iced
// `listen_with`), so the live keymap is mirrored through these globals —
// `update` syncs them on every message, the listener reads them.
static LIVE_KEYBINDS: std::sync::Mutex<Vec<(Action, String)>> =
    std::sync::Mutex::new(Vec::new());
static LIVE_CAPTURE: std::sync::Mutex<Option<Action>> = std::sync::Mutex::new(None);

/// Mirror the app's keymap into the global the keyboard listener reads.
pub fn sync_live(bindings: &Bindings, capture: Option<Action>) {
    if let Ok(mut slot) = LIVE_KEYBINDS.lock() {
        *slot = bindings.clone();
    }
    if let Ok(mut slot) = LIVE_CAPTURE.lock() {
        *slot = capture;
    }
}

pub fn live() -> (Bindings, Option<Action>) {
    let binds = LIVE_KEYBINDS.lock().map(|b| b.clone()).unwrap_or_default();
    let capture = LIVE_CAPTURE.lock().ok().and_then(|c| *c);
    (binds, capture)
}

/// Default keymap.
pub fn defaults() -> Bindings {
    Action::ALL
        .iter()
        .map(|a| (*a, default_of(*a).to_string()))
        .collect()
}

pub fn default_of(a: Action) -> &'static str {
    match a {
        Action::Copy => "ctrl+shift+c",
        Action::Paste => "ctrl+shift+v",
        Action::Search => "ctrl+f",
        Action::QuickToggle => "ctrl+`",
        Action::NewTerm => "ctrl+shift+t",
        Action::CloseTerm => "ctrl+shift+w",
        Action::Clear => "ctrl+shift+k",
        Action::NextTab => "ctrl+shift+right",
        Action::PrevTab => "ctrl+shift+left",
        Action::Settings => "ctrl+shift+s",
        Action::ScrollUp => "shift+up",
        Action::ScrollDown => "shift+down",
        Action::ScrollPageUp => "shift+pageup",
        Action::ScrollPageDown => "shift+pagedown",
        Action::ScrollHome => "shift+home",
        Action::ScrollEnd => "shift+end",
    }
}

/// Does this key press match the combo? `key_text` is the lowercased
/// printable character of the key (empty for named keys).
pub fn matches(combo: &str, mods: Modifiers, key: &Key) -> bool {
    let mut want_ctrl = false;
    let mut want_shift = false;
    let mut want_alt = false;
    let mut key_part = "";
    for part in combo.split('+') {
        match part {
            "ctrl" => want_ctrl = true,
            "shift" => want_shift = true,
            "alt" => want_alt = true,
            other => key_part = other,
        }
    }
    if want_ctrl != mods.control() || want_shift != mods.shift() || want_alt != mods.alt() {
        return false;
    }
    // The key part must match exactly (lowercased character or named key).
    match key {
        Key::Character(s) => {
            s.to_lowercase().starts_with(key_part)
                || s.to_lowercase().chars().next().is_some_and(|c| c.to_string() == key_part)
        }
        Key::Named(named) => named_key_name(*named) == key_part,
        _ => false,
    }
}

/// Name used inside combo strings for named keys we allow binding.
pub fn named_key_name(named: iced::keyboard::key::Named) -> &'static str {
    use iced::keyboard::key::Named as N;
    match named {
        N::ArrowLeft => "left",
        N::ArrowRight => "right",
        N::ArrowUp => "up",
        N::ArrowDown => "down",
        N::Tab => "tab",
        N::Enter => "enter",
        N::Backspace => "backspace",
        N::Delete => "delete",
        N::Home => "home",
        N::End => "end",
        N::PageUp => "pageup",
        N::PageDown => "pagedown",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Modifiers};

    fn key(c: char) -> Key {
        Key::Character(c.to_string().into())
    }

    #[test]
    fn default_keymap_covers_every_action() {
        let bindings = defaults();
        assert_eq!(bindings.len(), Action::ALL.len());
        for (action, combo) in &bindings {
            assert!(!combo.is_empty(), "{} has no default combo", action.id());
        }
    }

    #[test]
    fn matches_requires_exact_modifiers() {
        let combo = "ctrl+shift+c";
        assert!(matches(combo, Modifiers::CTRL | Modifiers::SHIFT, &key('c')));
        assert!(matches(combo, Modifiers::CTRL | Modifiers::SHIFT, &key('C')));
        assert!(!matches(combo, Modifiers::CTRL, &key('c')));
        assert!(!matches(combo, Modifiers::SHIFT, &key('c')));
        assert!(!matches(combo, Modifiers::empty(), &key('c')));
        assert!(!matches(combo, Modifiers::CTRL | Modifiers::SHIFT, &key('v')));
        assert!(!matches(
            combo,
            Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
            &key('c')
        ));
    }

    #[test]
    fn matches_named_keys() {
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches("ctrl+shift+right", mods, &Key::Named(Named::ArrowRight)));
        assert!(!matches("ctrl+shift+right", mods, &Key::Named(Named::ArrowLeft)));
        assert!(matches("ctrl+`", Modifiers::CTRL, &Key::Character("`".into())));
    }

    #[test]
    fn named_key_names() {
        assert_eq!(named_key_name(Named::ArrowLeft), "left");
        assert_eq!(named_key_name(Named::PageDown), "pagedown");
        assert_eq!(named_key_name(Named::Pause), "");
    }
}
