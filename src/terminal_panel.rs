// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Terminal Panel — Complete functional terminal emulator.
//!
//! Mirrors Kortina's terminal.rs exactly:
//! - Tab management with real data
//! - Full keyboard input → PTY
//! - Mouse selection
//! - Resize handling with debounce
//! - Content sensor for grid sizing

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, mouse_area, row, scrollable, stack, text};
use iced::{Alignment, Color, Element, Event, Length, Pixels, Subscription, Task};
use std::path::Path;

use crate::icons::Icon;
use crate::styles;
use crate::theme;

/// Tab drag lift-in duration (ms).
const TAB_LIFT_MS: f32 = 110.0;
/// Post-drop settle pulse duration (ms).
const TAB_SETTLE_MS: f32 = 200.0;
/// Terminal content switch slide duration (ms).
const CONTENT_SWITCH_MS: f32 = 200.0;
/// Hover delay before the overflow drag strip extends (ms).
const STRIP_SHOW_DELAY_MS: f32 = 350.0;
/// Grace period after the mouse leaves before the strip retracts (ms).
const STRIP_HIDE_DELAY_MS: f32 = 200.0;
/// Window grow/shrink animation duration (ms).
const STRIP_GROW_MS: f32 = 200.0;
/// Drag strip zone height (px) — reserved above the titlebar while
/// tabs overflow.
pub const STRIP_ZONE_H: f32 = 44.0;

/// Eased drag-lift amount (0..1) for the currently dragged tab.
pub fn tab_lift(app: &TerminalPanel) -> f32 {
    let p = (app.tab_drag_t / TAB_LIFT_MS).clamp(0.0, 1.0);
    1.0 - (1.0 - p) * (1.0 - p) * (1.0 - p)
}

/// Remaining settle-pulse amount (0..1) for a tab index, if pulsing.
pub fn tab_pulse(app: &TerminalPanel, idx: usize) -> f32 {
    match app.tab_settle {
        Some((i, t)) if i == idx => (1.0 - t / TAB_SETTLE_MS).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// The shell binary Korterm will actually spawn for `pref`.
///
/// An empty preference (or the "系统默认" placeholder) means "use the
/// user's login shell", which is what the PTY layer does. Settings →
/// Shell shows this resolved path so the status matches reality.
pub fn resolve_shell_path(pref: &str) -> String {
    let pref = pref.trim();
    if pref.is_empty() || pref == "系统默认" {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    } else if pref.contains('/') {
        pref.to_string()
    } else {
        // Bare name: look it up on PATH, fall back to the name itself so
        // status detection still classifies the family correctly.
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .map(|dir| Path::new(dir).join(pref))
            .find(|c| c.is_file())
            .map(|c| c.display().to_string())
            .unwrap_or_else(|| format!("/bin/{pref}"))
    }
}

/// What a close is waiting on confirmation for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseTarget {
    /// The whole window.
    Window,
    /// One tab, by index at the time the prompt was raised.
    Tab(usize),
}

/// What a tab hosts: a live terminal or the (single) settings page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionKind {
    Terminal,
    Settings,
}

pub struct TerminalSession {
    /// Stable id (spawn sequence number) — used as the FLIP tween key so
    /// tabs slide from their own old position when slots swap.
    pub id: usize,
    pub kind: SessionKind,
    pub title: String,
    /// Auto title prefix, e.g. `"shell"` — the displayed number is always
    /// the tab's current position, so closed tabs don't burn numbers.
    pub title_base: String,
    /// Whether the user renamed this tab (freezes the auto number).
    pub renamed: bool,
    pub term: terminal::Terminal,
}

impl TerminalSession {
    /// Title shown on the tab. Auto-named tabs are numbered by their
    /// current position (1-based); renamed tabs keep the custom name.
    /// The settings tab always shows a fixed label.
    pub fn display_title(&self, idx: usize) -> String {
        if self.kind == SessionKind::Settings {
            return "设置".to_string();
        }
        if self.renamed {
            self.title.clone()
        } else {
            format!("{} ({})", self.title_base, idx + 1)
        }
    }
}

/// Where the mouse is relative to the tab strip zone of the titlebar.
/// `None` = mouse outside the titlebar.
pub type TitlebarZone = Option<f32>;

/// Which dropdown/menu a close animation belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuKind {
    ShellSelector,
    Actions,
}

/// Which context menu is open (terminal body vs a tab).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuCtx {
    Terminal,
    Tab(usize),
}

/// Close animations in progress (elapsed ms per menu kind).
#[derive(Default)]
pub struct MenuClosing {
    pub shell: Option<f32>,
    pub actions: Option<f32>,
    /// Context menu close anim: (kind, last position, elapsed ms).
    pub context: Option<(MenuCtx, (f32, f32), f32)>,
}

impl MenuClosing {
    pub fn any(&self) -> bool {
        self.shell.is_some() || self.actions.is_some() || self.context.is_some()
    }
}


/// State of an in-progress tab drag (titlebar tabs and sidebar tabs).
#[derive(Clone, Copy, Debug)]
pub struct TabDrag {
    /// Current index of the dragged tab (updated live while reordering).
    pub idx: usize,
    /// Whether the tab has actually been moved — a plain click (no reorder)
    /// selects the tab on release; a real drag does not.
    pub reordered: bool,
}

pub struct TerminalPanel {
    pub glow: crate::glow::Glow,
    pub terminals: Vec<TerminalSession>,
    pub active_terminal: Option<usize>,
    pub terminal_focused: bool,
    pub term_shell_selector_open: bool,
    pub term_actions_menu_open: bool,
    /// Terminal right-click context menu, positioned at the click point.
    pub term_context_menu: Option<(MenuCtx, (f32, f32))>,
    /// Dropdown expand animation: elapsed milliseconds since open.
    pub term_menu_anim_t: f32,
    /// Whether the dropdown expand animation is running.
    pub term_menu_animating: bool,
    /// Close animations in progress (menu stays visible while shrinking).
    pub menu_closing: MenuClosing,
    /// Shared FLIP position tracker for tab swap slides.
    pub tab_tweens: crate::animation::SharedTweenTracker,
    /// Last seen keyboard modifiers (for ctrl+click link/file jump).
    pub modifiers: iced::keyboard::Modifiers,
    /// Last mouse position over the terminal content area.
    pub term_mouse: (f32, f32),
    /// Live tab drag state (None = no drag in progress).
    pub term_tab_drag: Option<TabDrag>,
    /// Index of the tab currently hovered by the mouse.
    pub tab_hover: Option<usize>,
    /// Drag lift-in animation progress (elapsed ms, 0..TAB_LIFT_MS).
    pub tab_drag_t: f32,
    /// Post-drop settle pulse: (tab index, elapsed ms).
    pub tab_settle: Option<(usize, f32)>,
    /// Terminal content switch slide: (from idx, to idx, elapsed ms).
    /// `to` is right of `from` → content slides left, and vice versa.
    pub content_anim: Option<(usize, usize, f32)>,
    /// Whether the mouse is over the overflow drag strip itself.
    pub titlebar_grip_hover: bool,
    /// Mouse x over the titlebar (None = outside). Strip reveal is
    /// suppressed over the corner button zones (traffic lights / actions).
    pub titlebar_hover_x: TitlebarZone,
    /// Whether the drag strip is currently revealed (hover-driven).
    pub strip_visible: bool,
    /// Hover start instant — strip extends after `STRIP_SHOW_DELAY_MS`.
    pub strip_show_at: Option<std::time::Instant>,
    /// When the mouse left: retract the strip after `STRIP_HIDE_DELAY_MS`.
    pub strip_hide_at: Option<std::time::Instant>,
    /// Strip reveal progress 0..1 (animated).
    pub strip_anim_t: f32,
    /// Extra window height currently requested for the strip (animated).
    pub strip_extra_applied: f32,
    /// Window height with the strip fully retracted (resize base).
    pub strip_base_h: f32,
    /// Horizontal scroll offset of the titlebar tab strip (px).
    pub tabs_scroll_x: f32,
    /// Vertical scroll offset of the sidebar tab list (px).
    pub sidebar_scroll_y: f32,
    /// Whether tabs overflow the titlebar (drag strip becomes available).
    pub tabs_overflow: bool,
    /// Inline tab-rename editor: (tab index, current draft).
    pub tab_rename: Option<(usize, String)>,
    /// Current IME composition (preedit) shown over-the-spot.
    pub ime_preedit: String,
    pub term_statusbar_visible: bool,
    pub term_tabs_vertical: bool,
    pub term_tab_width: f32,
    pub terminal_seq: usize,
    pub window_size: (f32, f32),
    pub terminal_height: f32,
    pub term_content_size: (f32, f32),
    pub term_selecting: bool,
    pub term_pty_resize_pending: bool,
    pub main_window: Option<iced::window::Id>,
    /// Last measured monospace character width, used to detect when the
    /// renderer has finally measured the font and we need to reflow.
    pub term_last_char_w: f32,
    /// Remembered shell preference ("" = system default).
    pub term_default_shell: String,
    /// Cached snapshot of the shell-integration state shown in Settings →
    /// Shell. Refreshed on demand (settings open / section change /
    /// enable / disable) instead of every frame.
    pub shell_status: crate::shell_integration::Status,
    /// Result of the last enable/disable attempt, shown in that section.
    pub shell_notice: Option<String>,
    /// Pending destructive close, shown as a confirmation dialog when a
    /// program is still running in that terminal.
    pub close_confirm: Option<CloseTarget>,
    /// Whether the sidebar resize handle is being dragged.
    pub sidebar_dragging: bool,
    /// Active settings section (left nav).
    pub settings_section: crate::settings::Section,
    /// Open color picker (which glow), if any.
    pub picker: Option<u8>,
    /// Picker HSV state (hue, saturation, value).
    pub picker_hsv: (f32, f32, f32),
    /// Color at the moment the picker was opened (cancel restores it).
    pub picker_backup: Option<(u8, iced::Color)>,
    /// Picker drag target: 0 = none, 1 = SV field, 2 = hue strip.
    pub picker_drag: u8,
    /// Settings page entrance animation progress (0..MENU_ANIM_MS).
    pub settings_anim_t: f32,
    /// Shell dropdown menu state (appearance section).
    pub shell_menu_open: bool,
    pub shell_menu_anim_t: f32,
    /// Color picker popover entrance animation progress.
    pub picker_anim_t: f32,
    /// iOS-toggle animation progress per switch ([statusbar, vertical]).
    pub toggle_progress: [f32; 2],
    /// Toggle switch animation targets (0 = off, 1 = on).
    pub toggle_anim_target: [usize; 2],
    /// Search bar state (open = visible, closing = play collapse anim).
    pub search_open: bool,
    pub search_closing: bool,
    pub search_anim_t: f32,
    /// True while a debounced search apply is scheduled.
    pub search_pending: bool,
    pub search_query: String,
    /// User-configurable shortcuts + capture state for rebinding.
    pub keybinds: crate::keybinds::Bindings,
    pub keybind_capture: Option<crate::keybinds::Action>,
    /// Hex color text drafts for the two glow cards.
    pub glow_hex_top: String,
    pub glow_hex_bottom: String,
}

pub enum Message {
    TermNew,
    TermNewShell(String),
    TermClose(usize),
    TermSelect(usize),
    /// Select the next/previous tab (wrapping).
    TermSelectNext,
    TermSelectPrev,
    /// Close the currently active terminal.
    TermCloseActive,
    TermSelectPress,
    TermSelectMove(f32, f32),
    TermSelectRelease,
    TermTabPress(usize),
    TermTabHover(usize, f32, f32),
    TermTabExit(usize),
    TermTabRelease,
    /// Mouse entered/left the top drag strip of the titlebar.
    TitlebarGripHover(bool),
    /// Wheel scroll over the titlebar tab strip (native horizontal
    /// scrollables ignore plain vertical wheel, so we scroll manually).
    TitlebarHover(TitlebarZone),
    TitlebarScroll(iced::mouse::ScrollDelta),
    /// Press on the sidebar resize handle → start dragging the divider.
    SidebarDragStart,
    /// Open the terminal search bar (Ctrl+F).
    SearchOpen,
    /// Search query changed — debounce, then re-run the buffer search.
    SearchInput(String),
    /// Apply the pending search query (debounce tick).
    SearchApply,
    /// Jump to the next/previous match.
    SearchNav(bool),
    /// Close the search bar and clear highlights.
    SearchClose,
    /// Open the file:line target of the current match (xdg-open).
    SearchOpenTarget,
    /// Open the settings tab.
    SettingsOpen,
    /// Close the settings tab.
    SettingsClose,
    /// Switch the active settings section (left nav).
    SettingsSection(crate::settings::Section),
    /// Cycle the default shell preference (appearance section).
    ShellCycle,
    /// Pick a shell from the dropdown.
    ShellSelect(String),
    /// Pick one of the preset glow colors (`which`: 0 = top, 1 = bottom).
    GlowPick(u8, u32),
    /// Open the color picker popover for a glow.
    GlowPickerOpen(u8),
    /// Close the color picker popover.
    GlowPickerClose,
    /// SV field pressed/dragged (view-relative coords).
    GlowSvDrag(u8, f32, f32),
    /// Hue strip pressed/dragged (view-relative x).
    GlowHueDrag(u8, f32),
    /// Glow brightness set continuously (0..100).
    GlowIntensitySet(u8, f32),
    /// Hex color text changed in a glow card.
    GlowHexInput(u8, String),
    /// Hex color text submitted (Enter) in a glow card.
    GlowHexSubmit(u8),
    /// Confirm the color picker selection (save + close).
    GlowPickerApply,
    /// Restore default glow colors + intensity.
    GlowReset,
    /// Start rebinding a shortcut (capture the next key press).
    KeyBindListen(crate::keybinds::Action),
    /// A combo was captured for the action being rebound.
    KeyBindSet(crate::keybinds::Action, String),
    /// Cancel shortcut rebinding.
    KeyBindCancel,
    /// Right-click on a titlebar/sidebar tab → open its context menu.
    TermTabRightClick(usize),
    /// Context menu "重命名" → open the rename editor.
    TabRenameStart(usize),
    /// Rename editor text changed.
    TabRenameInput(String),
    /// Rename editor submitted (Enter).
    TabRenameSubmit,
    /// IME (fcitx5/ibus) committed text over a terminal zone.
    ImeCommit(String),
    /// IME composition (preedit) text updated.
    ImePreedit(String),
    /// Ctrl+` — spawn/toggle the standalone quick terminal window.
    QuickToggle,
    RightClickTerminal,
    TermClear,
    TermToggleStatusbar,
    TermToggleTabsVertical,
    TermShowShellSelector,
    TermShowActionsMenu,
    TermCloseMenus,
    TermCloseAll,
    TermWrite(Vec<u8>),
    TermResize(usize, usize, f32, f32),
    TermFlushPtyResize,
    TermCopy,
    TermPaste,
    TermSelectAll,
    /// Confirm (or cancel) the pending close.
    CloseConfirmAccept,
    CloseConfirmCancel,
    /// A file/folder was dropped onto the window from a file manager.
    /// The path is shell-quoted and written to the focused PTY.
    FileDropped(std::path::PathBuf),
    /// Shell section: write/remove the Korterm rc block.
    ShellIntegrationEnable,
    ShellIntegrationDisable,
    /// Copy the apt command for missing shell components.
    ShellCopyInstallCmd,
    /// Type that command into the focused terminal (no auto-run).
    ShellPasteInstallCmd,
    ToggleTerminal,
    CloseWindow,
    MinimizeWindow,
    ToggleMaximize,
    DragTitlebar,
    MainWindowReady(iced::window::Id),
    Tick,
    /// True no-op (unmapped events, empty clipboard reads, …). `Tick`
    /// advances cursor animations, so it must never be used as a
    /// fallback — every keypress would visibly jerk the cursor.
    Nop,
    /// Real scroll offset reported by the titlebar tab strip.
    TitlebarScrolled(f32),
    /// Real scroll offset reported by the sidebar tab list.
    SidebarScrolled(f32),
    /// Re-assert the saved tab-list scroll after a layout switch.
    RestoreTabsScroll,
    TermPump,
    AnimTick,
    /// A terminal was spawned asynchronously (from `create_terminal`).
    /// The Terminal itself travels through the `PENDING_TERMS` handoff
    /// queue (keyed by `seq`) so this message stays cheaply cloneable —
    /// widgets like `mouse_area`/`text_input` require `Message: Clone`.
    TerminalSpawned { seq: usize, title: String, base: String },
    Event(iced::Event),
}

impl Clone for Message {
    fn clone(&self) -> Self {
        match self {
            Message::TermNew => Message::TermNew,
            Message::TermNewShell(s) => Message::TermNewShell(s.clone()),
            Message::TermClose(i) => Message::TermClose(*i),
            Message::TermSelect(i) => Message::TermSelect(*i),
            Message::TermSelectNext => Message::TermSelectNext,
            Message::TermSelectPrev => Message::TermSelectPrev,
            Message::TermCloseActive => Message::TermCloseActive,
            Message::TermSelectPress => Message::TermSelectPress,
            Message::TermSelectMove(x, y) => Message::TermSelectMove(*x, *y),
            Message::TermSelectRelease => Message::TermSelectRelease,
            Message::TermTabPress(i) => Message::TermTabPress(*i),
            Message::TermTabHover(i, x, y) => Message::TermTabHover(*i, *x, *y),
            Message::TermTabExit(i) => Message::TermTabExit(*i),
            Message::TermTabRelease => Message::TermTabRelease,
            Message::TitlebarGripHover(b) => Message::TitlebarGripHover(*b),
            Message::TitlebarHover(z) => Message::TitlebarHover(*z),
            Message::TitlebarScroll(d) => Message::TitlebarScroll(*d),
            Message::SidebarDragStart => Message::SidebarDragStart,
            Message::SearchOpen => Message::SearchOpen,
            Message::SearchInput(s) => Message::SearchInput(s.clone()),
            Message::SearchApply => Message::SearchApply,
            Message::SearchNav(f) => Message::SearchNav(*f),
            Message::SearchClose => Message::SearchClose,
            Message::SearchOpenTarget => Message::SearchOpenTarget,
            Message::SettingsOpen => Message::SettingsOpen,
            Message::SettingsClose => Message::SettingsClose,
            Message::SettingsSection(s) => Message::SettingsSection(*s),
            Message::ShellCycle => Message::ShellCycle,
            Message::ShellSelect(s) => Message::ShellSelect(s.clone()),
            Message::GlowPick(w, c) => Message::GlowPick(*w, *c),
            Message::GlowPickerOpen(w) => Message::GlowPickerOpen(*w),
            Message::GlowPickerClose => Message::GlowPickerClose,
            Message::GlowSvDrag(w, x, y) => Message::GlowSvDrag(*w, *x, *y),
            Message::GlowHueDrag(w, x) => Message::GlowHueDrag(*w, *x),
            Message::GlowIntensitySet(w, v) => Message::GlowIntensitySet(*w, *v),
            Message::GlowHexInput(w, s) => Message::GlowHexInput(*w, s.clone()),
            Message::GlowHexSubmit(w) => Message::GlowHexSubmit(*w),
            Message::GlowPickerApply => Message::GlowPickerApply,
            Message::GlowReset => Message::GlowReset,
            Message::KeyBindListen(a) => Message::KeyBindListen(*a),
            Message::KeyBindSet(a, c) => Message::KeyBindSet(*a, c.clone()),
            Message::KeyBindCancel => Message::KeyBindCancel,
            Message::TermTabRightClick(i) => Message::TermTabRightClick(*i),
            Message::TabRenameStart(i) => Message::TabRenameStart(*i),
            Message::TabRenameInput(s) => Message::TabRenameInput(s.clone()),
            Message::TabRenameSubmit => Message::TabRenameSubmit,
            Message::ImeCommit(s) => Message::ImeCommit(s.clone()),
            Message::ImePreedit(s) => Message::ImePreedit(s.clone()),
            Message::QuickToggle => Message::QuickToggle,
            Message::RightClickTerminal => Message::RightClickTerminal,
            Message::TermClear => Message::TermClear,
            Message::TermToggleStatusbar => Message::TermToggleStatusbar,
            Message::TermToggleTabsVertical => Message::TermToggleTabsVertical,
            Message::TermShowShellSelector => Message::TermShowShellSelector,
            Message::TermShowActionsMenu => Message::TermShowActionsMenu,
            Message::TermCloseMenus => Message::TermCloseMenus,
            Message::TermCloseAll => Message::TermCloseAll,
            Message::TermWrite(b) => Message::TermWrite(b.clone()),
            Message::TermResize(c, r, w, h) => Message::TermResize(*c, *r, *w, *h),
            Message::TermFlushPtyResize => Message::TermFlushPtyResize,
            Message::TermCopy => Message::TermCopy,
            Message::TermPaste => Message::TermPaste,
            Message::CloseConfirmAccept => Message::CloseConfirmAccept,
            Message::CloseConfirmCancel => Message::CloseConfirmCancel,
            Message::FileDropped(p) => Message::FileDropped(p.clone()),
            Message::ShellIntegrationEnable => Message::ShellIntegrationEnable,
            Message::ShellIntegrationDisable => Message::ShellIntegrationDisable,
            Message::ShellCopyInstallCmd => Message::ShellCopyInstallCmd,
            Message::ShellPasteInstallCmd => Message::ShellPasteInstallCmd,
            Message::TermSelectAll => Message::TermSelectAll,
            Message::ToggleTerminal => Message::ToggleTerminal,
            Message::CloseWindow => Message::CloseWindow,
            Message::MinimizeWindow => Message::MinimizeWindow,
            Message::ToggleMaximize => Message::ToggleMaximize,
            Message::DragTitlebar => Message::DragTitlebar,
            Message::MainWindowReady(id) => Message::MainWindowReady(*id),
            Message::Tick => Message::Tick,
            Message::Nop => Message::Nop,
            Message::TitlebarScrolled(x) => Message::TitlebarScrolled(*x),
            Message::SidebarScrolled(y) => Message::SidebarScrolled(*y),
            Message::RestoreTabsScroll => Message::RestoreTabsScroll,
            Message::TermPump => Message::TermPump,
            Message::AnimTick => Message::AnimTick,
            Message::TerminalSpawned { seq, title, base } => Message::TerminalSpawned {
                seq: *seq,
                title: title.clone(),
                base: base.clone(),
            },
            Message::Event(e) => Message::Event(e.clone()),
        }
    }
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::TermNew => write!(f, "TermNew"),
            Message::TermNewShell(s) => write!(f, "TermNewShell({s})"),
            Message::TermClose(i) => write!(f, "TermClose({i})"),
            Message::TermSelect(i) => write!(f, "TermSelect({i})"),
            Message::TermSelectNext => write!(f, "TermSelectNext"),
            Message::TermSelectPrev => write!(f, "TermSelectPrev"),
            Message::TermCloseActive => write!(f, "TermCloseActive"),
            Message::TermSelectPress => write!(f, "TermSelectPress"),
            Message::TermSelectMove(x, y) => write!(f, "TermSelectMove({x}, {y})"),
            Message::TermSelectRelease => write!(f, "TermSelectRelease"),
            Message::TermTabPress(i) => write!(f, "TermTabPress({i})"),
            Message::TermTabHover(i, x, y) => write!(f, "TermTabHover({i}, {x}, {y})"),
            Message::TermTabExit(i) => write!(f, "TermTabExit({i})"),
            Message::TermTabRelease => write!(f, "TermTabRelease"),
            Message::TitlebarGripHover(b) => write!(f, "TitlebarGripHover({b})"),
            Message::TitlebarHover(z) => write!(f, "TitlebarHover({z:?})"),
            Message::TitlebarScroll(d) => write!(f, "TitlebarScroll({d:?})"),
            Message::SidebarDragStart => write!(f, "SidebarDragStart"),
            Message::SearchOpen => write!(f, "SearchOpen"),
            Message::SearchInput(s) => write!(f, "SearchInput({s:?})"),
            Message::SearchApply => write!(f, "SearchApply"),
            Message::SearchNav(fwd) => write!(f, "SearchNav({fwd:?})"),
            Message::SearchClose => write!(f, "SearchClose"),
            Message::SearchOpenTarget => write!(f, "SearchOpenTarget"),
            Message::SettingsOpen => write!(f, "SettingsOpen"),
            Message::SettingsClose => write!(f, "SettingsClose"),
            Message::SettingsSection(s) => write!(f, "SettingsSection({s:?})"),
            Message::ShellCycle => write!(f, "ShellCycle"),
            Message::ShellSelect(s) => write!(f, "ShellSelect({s})"),
            Message::GlowPick(w, c) => write!(f, "GlowPick({w}, {c:#08x})"),
            Message::GlowPickerOpen(w) => write!(f, "GlowPickerOpen({w})"),
            Message::GlowPickerClose => write!(f, "GlowPickerClose"),
            Message::GlowSvDrag(w, x, y) => write!(f, "GlowSvDrag({w}, {x:.2}, {y:.2})"),
            Message::GlowHueDrag(w, x) => write!(f, "GlowHueDrag({w}, {x:.1})"),
            Message::GlowIntensitySet(w, v) => write!(f, "GlowIntensitySet({w}, {v})"),
            Message::GlowHexInput(w, s) => write!(f, "GlowHexInput({w}, {s:?})"),
            Message::GlowHexSubmit(w) => write!(f, "GlowHexSubmit({w})"),
            Message::GlowPickerApply => write!(f, "GlowPickerApply"),
            Message::GlowReset => write!(f, "GlowReset"),
            Message::KeyBindListen(a) => write!(f, "KeyBindListen({a:?})"),
            Message::KeyBindSet(a, c) => write!(f, "KeyBindSet({a:?}, {c})"),
            Message::KeyBindCancel => write!(f, "KeyBindCancel"),
            Message::TermTabRightClick(i) => write!(f, "TermTabRightClick({i})"),
            Message::TabRenameStart(i) => write!(f, "TabRenameStart({i})"),
            Message::TabRenameInput(s) => write!(f, "TabRenameInput({s:?})"),
            Message::TabRenameSubmit => write!(f, "TabRenameSubmit"),
            Message::ImeCommit(s) => write!(f, "ImeCommit({s:?})"),
            Message::ImePreedit(s) => write!(f, "ImePreedit({s:?})"),
            Message::QuickToggle => write!(f, "QuickToggle"),
            Message::RightClickTerminal => write!(f, "RightClickTerminal"),
            Message::TermClear => write!(f, "TermClear"),
            Message::TermToggleStatusbar => write!(f, "TermToggleStatusbar"),
            Message::TermToggleTabsVertical => write!(f, "TermToggleTabsVertical"),
            Message::TermShowShellSelector => write!(f, "TermShowShellSelector"),
            Message::TermShowActionsMenu => write!(f, "TermShowActionsMenu"),
            Message::TermCloseMenus => write!(f, "TermCloseMenus"),
            Message::TermCloseAll => write!(f, "TermCloseAll"),
            Message::TermWrite(_) => write!(f, "TermWrite(...)"),
            Message::TermResize(c, r, w, h) => write!(f, "TermResize({c}, {r}, {w}, {h})"),
            Message::TermFlushPtyResize => write!(f, "TermFlushPtyResize"),
            Message::TermCopy => write!(f, "TermCopy"),
            Message::TermPaste => write!(f, "TermPaste"),
            Message::CloseConfirmAccept => write!(f, "CloseConfirmAccept"),
            Message::CloseConfirmCancel => write!(f, "CloseConfirmCancel"),
            Message::FileDropped(p) => write!(f, "FileDropped({})", p.display()),
            Message::ShellIntegrationEnable => write!(f, "ShellIntegrationEnable"),
            Message::ShellIntegrationDisable => write!(f, "ShellIntegrationDisable"),
            Message::ShellCopyInstallCmd => write!(f, "ShellCopyInstallCmd"),
            Message::ShellPasteInstallCmd => write!(f, "ShellPasteInstallCmd"),
            Message::TermSelectAll => write!(f, "TermSelectAll"),
            Message::ToggleTerminal => write!(f, "ToggleTerminal"),
            Message::CloseWindow => write!(f, "CloseWindow"),
            Message::MinimizeWindow => write!(f, "MinimizeWindow"),
            Message::ToggleMaximize => write!(f, "ToggleMaximize"),
            Message::DragTitlebar => write!(f, "DragTitlebar"),
            Message::MainWindowReady(id) => write!(f, "MainWindowReady({id:?})"),
            Message::Tick => write!(f, "Tick"),
            Message::Nop => write!(f, "Nop"),
            Message::TitlebarScrolled(x) => write!(f, "TitlebarScrolled({x})"),
            Message::SidebarScrolled(y) => write!(f, "SidebarScrolled({y})"),
            Message::RestoreTabsScroll => write!(f, "RestoreTabsScroll"),
            Message::TermPump => write!(f, "TermPump"),
            Message::AnimTick => write!(f, "AnimTick"),
            Message::TerminalSpawned { seq, title, base, .. } => {
                write!(f, "TerminalSpawned {{ seq: {seq}, title: {title}, base: {base} }}")
            }
            Message::Event(e) => write!(f, "Event({e:?})"),
        }
    }
}

impl TerminalPanel {
    pub fn new() -> (Self, Task<Message>) {
        // Restore persisted preferences (statusbar, tab layout, sidebar
        // width, shell choice); missing file → defaults.
        let cfg = crate::config::load();
        let panel = Self {
            glow: crate::glow::Glow::new(),
            terminals: Vec::new(),
            active_terminal: None,
            terminal_focused: false,
            term_shell_selector_open: false,
            term_actions_menu_open: false,
            term_context_menu: None,
            term_menu_anim_t: 0.0,
            term_menu_animating: false,
            menu_closing: MenuClosing::default(),
            tab_tweens: std::rc::Rc::new(std::cell::RefCell::new(
                crate::animation::TweenTracker::new(),
            )),
            modifiers: iced::keyboard::Modifiers::empty(),
            term_mouse: (0.0, 0.0),
            term_tab_drag: None,
            tab_hover: None,
            tab_drag_t: 0.0,
            tab_settle: None,
            content_anim: None,
            titlebar_grip_hover: false,
            titlebar_hover_x: None,
            strip_visible: false,
            strip_show_at: None,
            strip_hide_at: None,
            strip_anim_t: 0.0,
            strip_extra_applied: 0.0,
            strip_base_h: 0.0,
            tabs_scroll_x: 0.0,
            sidebar_scroll_y: 0.0,
            tabs_overflow: false,
            tab_rename: None,
            ime_preedit: String::new(),
            term_statusbar_visible: cfg.statusbar_visible,
            term_tabs_vertical: cfg.tabs_vertical,
            term_tab_width: cfg.sidebar_width,
            // The initial terminal is spawned with seq 1 below, so the
            // counter must start there — otherwise the first manual
            // "new terminal" also gets seq 1 and two tabs share a FLIP
            // tween key (the "second tab pushes the first" bug).
            terminal_seq: 1,
            window_size: (1000.0, 700.0),
            terminal_height: 656.0,
            term_content_size: (940.0, 596.0),
            term_selecting: false,
            term_pty_resize_pending: false,
            main_window: None,
            term_last_char_w: 0.0,
            shell_status: crate::shell_integration::status(&resolve_shell_path(&cfg.shell)),
            term_default_shell: cfg.shell,
            shell_notice: None,
            close_confirm: None,
            sidebar_dragging: false,
            settings_section: crate::settings::Section::Appearance,
            picker: None,
            picker_hsv: (210.0, 0.7, 1.0),
            picker_backup: None,
            picker_drag: 0,
            settings_anim_t: 0.0,
            shell_menu_open: false,
            shell_menu_anim_t: 0.0,
            picker_anim_t: 0.0,
            // Settings toggles start at the restored config values, not
            // hardcoded "on" — otherwise the switches visibly snap when
            // the first AnimTick pulls them to the real target.
            toggle_progress: [
                cfg.statusbar_visible as u8 as f32,
                cfg.tabs_vertical as u8 as f32,
            ],
            toggle_anim_target: [
                cfg.statusbar_visible as usize,
                cfg.tabs_vertical as usize,
            ],
            search_open: false,
            search_closing: false,
            search_anim_t: 0.0,
            search_pending: false,
            search_query: String::new(),
            keybinds: {
                let mut kb = crate::keybinds::defaults();
                // Apply persisted overrides from the config file.
                let cfg = crate::config::load();
                for (a, combo) in kb.iter_mut() {
                    if let Some(saved) = cfg.keybind_of(a.id()) {
                        *combo = saved;
                    }
                }
                kb
            },
            keybind_capture: None,
            glow_hex_top: String::new(),
            glow_hex_bottom: String::new(),
        };
        // Get the window ID as soon as the window is created
        let init = iced::window::latest().map(|opt| {
            match opt {
                Some(id) => Message::MainWindowReady(id),
                None => Message::Nop,
            }
        });
        // Create first terminal asynchronously (honor the remembered shell)
        let shell_pref = panel.term_default_shell.clone();
        let shell_opt = (!shell_pref.is_empty()).then_some(shell_pref);
        let seq0 = 1usize;
        let spawn = Task::perform(
            async move {
                let term =
                    terminal::Terminal::with_shell_async(80, 24, shell_opt.as_deref(), None)
                        .await;
                PENDING_TERMS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((seq0, Box::new(term)));
            },
            |_| {
                Message::TerminalSpawned {
                    seq: 1,
                    title: "shell (1)".to_string(),
                    base: "shell".to_string(),
                }
            },
        );
        (panel, iced::Task::batch([init, spawn]))
    }

    /// `true` when the terminal in front has something running.
    fn active_term_busy(&mut self) -> bool {
        self.active_term_mut()
            .map(|t| t.term.is_busy())
            .unwrap_or(false)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Mirror the keymap for the capture-free keyboard listener.
        crate::keybinds::sync_live(&self.keybinds, self.keybind_capture);
        match message {
            Message::TermNew => {
                self.close_shell_selector();
                self.close_context_menu();
                // Honor the remembered shell preference, if any.
                let pref = self.term_default_shell.clone();
                let pref = (!pref.is_empty()).then_some(pref);
                return create_terminal(self, pref.as_deref());
            }
            Message::TermNewShell(shell) => {
                self.close_shell_selector();
                self.close_context_menu();
                // Remember the choice for future "new terminal" actions.
                self.term_default_shell = shell.clone();
                self.save_config();
                return create_terminal(self, Some(&shell));
            }
            Message::TerminalSpawned { seq, title, base } => {
                // Pick the terminal up from the handoff queue. If it's gone
                // (message was cloned and processed twice), just skip.
                let Some(term) = take_pending_term(seq) else {
                    return Task::none();
                };
                self.terminals.push(TerminalSession {
                    id: seq,
                    kind: SessionKind::Terminal,
                    title,
                    title_base: base,
                    renamed: false,
                    term: *term,
                });
                // Seed the FLIP tracker so the new tab slides in from under
                // the "+" button (first button of the right-side group,
                // centered ~129px from the right window edge).
                let (ww, _) = self.window_size;
                self.tab_tweens.borrow_mut().seed(
                    seq as u64,
                    (ww - 129.0, 6.0),
                );
                self.active_terminal = Some(self.terminals.len() - 1);
                self.reflow_active_terminal();
                return self.sync_drag_strip();
            }
            Message::TitlebarGripHover(hovered) => {
                self.titlebar_grip_hover = hovered;
                self.sync_strip_visibility();
            }
            Message::TitlebarHover(x) => {
                self.titlebar_hover_x = x;
                self.sync_strip_visibility();
            }
            Message::TitlebarScroll(delta) => {
                // Plain vertical wheel does not scroll horizontal
                // scrollables in iced (direction.align drops the y axis),
                // so map it manually. Use scroll_by so the scrollable
                // itself clamps to the real content extent — the old
                // clamp(0, ww) capped the strip at ~16 tabs on narrow
                // windows.
                let dx = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => -y * 60.0,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => -y,
                };
                return iced::widget::operation::scroll_by(
                    iced::widget::Id::new("titlebar-tabs"),
                    iced::widget::scrollable::AbsoluteOffset { x: dx, y: 0.0 },
                );
            }
            Message::TitlebarScrolled(x) => {
                self.tabs_scroll_x = x;
            }
            Message::SidebarScrolled(y) => {
                self.sidebar_scroll_y = y;
            }
            Message::SidebarDragStart => {
                self.sidebar_dragging = true;
            }
            Message::SearchOpen => {
                self.search_open = true;
                self.search_closing = false;
                self.search_anim_t = 0.0;
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "term-search",
                ));
            }
            Message::SearchInput(q) => {
                self.search_query = q;
                // Debounce: full-buffer search only after typing pauses.
                if !self.search_pending {
                    self.search_pending = true;
                    return Task::future(async {
                        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
                        Message::SearchApply
                    });
                }
            }
            Message::SearchApply => {
                self.search_pending = false;
                let q = self.search_query.clone();
                if let Some(term) = self.active_term_mut() {
                    term.term.search_set(&q);
                }
            }
            Message::SearchNav(forward) => {
                if let Some(term) = self.active_term_mut() {
                    term.term.search_next(forward);
                }
            }
            Message::SearchClose => {
                // Play the collapse animation, then really close.
                self.search_closing = true;
                self.search_anim_t = 0.0;
                self.search_query.clear();
                if let Some(term) = self.active_term_mut() {
                    term.term.search_set("");
                    term.term.scroll_to_bottom();
                }
            }
            Message::SearchOpenTarget => {
                if let Some(term) = self.active_term() {
                    if let Some(m) = term.term.search_current() {
                        if let Some(target) = classify_jump_target(&m.text) {
                            let _ = std::process::Command::new("xdg-open")
                                .arg(&target)
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .spawn();
                        }
                    }
                }
            }
            Message::SettingsOpen => {
                self.search_open = false;
                self.search_closing = false;
                self.keybind_capture = None;
                self.close_actions_menu();
                // Component availability can change between runs (the
                // user installs a plugin, edits .zshrc, …) — re-detect
                // when the page opens rather than only at startup.
                self.shell_status = crate::shell_integration::status(
                    &resolve_shell_path(&self.term_default_shell),
                );
                self.shell_notice = None;
                // Reuse the settings session if one exists; otherwise
                // create it as a real tab (draggable, closable like any
                // other tab).
                if let Some(idx) = self
                    .terminals
                    .iter()
                    .position(|s| s.kind == SessionKind::Settings)
                {
                    self.select_terminal(idx);
                } else {
                    self.terminal_seq += 1;
                    let seq = self.terminal_seq;
                    self.terminals.push(TerminalSession {
                        id: seq,
                        kind: SessionKind::Settings,
                        title: "设置".to_string(),
                        title_base: "设置".to_string(),
                        renamed: true,
                        term: terminal::Terminal::headless(80, 24),
                    });
                    self.settings_anim_t = 0.0;
                    self.select_terminal(self.terminals.len() - 1);
                }
            }
            Message::SettingsClose => {
                // Close the settings session tab, like any other tab.
                if let Some(idx) = self
                    .terminals
                    .iter()
                    .position(|s| s.kind == SessionKind::Settings)
                {
                    return self.update(Message::TermClose(idx));
                }
            }
            Message::SettingsSection(s) => {
                self.settings_section = s;
                self.picker = None;
                if s == crate::settings::Section::Shell {
                    self.shell_status = crate::shell_integration::status(
                        &resolve_shell_path(&self.term_default_shell),
                    );
                }
            }
            Message::ShellCycle => {
                // Toggle the shell dropdown menu (k-select style).
                self.shell_menu_open = !self.shell_menu_open;
                if self.shell_menu_open {
                    self.shell_menu_anim_t = 0.0;
                }
            }
            Message::ShellSelect(shell) => {
                self.shell_menu_open = false;
                self.shell_status =
                    crate::shell_integration::status(&resolve_shell_path(&shell));
                self.term_default_shell = if shell == "系统默认" {
                    String::new()
                } else {
                    shell
                };
                self.save_config();
            }
            Message::GlowPick(which, rgb) => {
                let color = iced::Color::from_rgb8(
                    ((rgb >> 16) & 0xFF) as u8,
                    ((rgb >> 8) & 0xFF) as u8,
                    (rgb & 0xFF) as u8,
                );
                match which {
                    0 => self.glow.blue = color,
                    _ => self.glow.amber = color,
                }
                // Keep the picker's HSV in sync with the preset.
                self.picker_hsv = crate::settings::rgb_to_hsv(color);
                self.glow.invalidate();
                self.save_config();
            }
            Message::GlowPickerOpen(which) => {
                let color = match which {
                    0 => self.glow.blue,
                    _ => self.glow.amber,
                };
                self.picker_hsv = crate::settings::rgb_to_hsv(color);
                self.picker_backup = Some((which, color));
                self.picker_anim_t = 0.0;
                self.picker = Some(which);
            }
            Message::GlowPickerClose => {
                // Cancel: restore the color from when the picker opened.
                if let Some((which, color)) = self.picker_backup.take() {
                    match which {
                        0 => self.glow.blue = color,
                        _ => self.glow.amber = color,
                    }
                    self.glow.invalidate();
                }
                self.picker = None;
                self.picker_drag = 0;
                self.save_config();
            }
            Message::GlowPickerApply => {
                self.picker_backup = None;
                self.picker = None;
                self.picker_drag = 0;
                self.glow_hex_top.clear();
                self.glow_hex_bottom.clear();
                self.save_config();
            }
            Message::GlowSvDrag(which, s, v) => {
                self.picker_hsv.1 = s;
                self.picker_hsv.2 = v;
                let color = crate::settings::hsv_to_rgb(
                    self.picker_hsv.0,
                    self.picker_hsv.1,
                    self.picker_hsv.2,
                );
                match which {
                    0 => self.glow.blue = color,
                    _ => self.glow.amber = color,
                }
                self.glow.invalidate();
                self.save_config();
            }
            Message::GlowHueDrag(which, h) => {
                self.picker_hsv.0 = h;
                let color =
                    crate::settings::hsv_to_rgb(h, self.picker_hsv.1, self.picker_hsv.2);
                match which {
                    0 => self.glow.blue = color,
                    _ => self.glow.amber = color,
                }
                self.glow.invalidate();
                self.save_config();
            }
            Message::GlowIntensitySet(which, v) => {
                let value = (v / 100.0).clamp(0.0, 1.0);
                match which {
                    0 => self.glow.intensity_top = value,
                    _ => self.glow.intensity_bottom = value,
                }
                self.glow.invalidate();
                self.save_config();
            }
            Message::GlowHexInput(which, text) => match which {
                0 => self.glow_hex_top = text,
                _ => self.glow_hex_bottom = text,
            },
            Message::GlowHexSubmit(which) => {
                let raw = match which {
                    0 => self.glow_hex_top.clone(),
                    _ => self.glow_hex_bottom.clone(),
                };
                let hex = raw.trim().trim_start_matches('#').to_string();
                if hex.len() == 6 {
                    if let Ok(rgb) = u32::from_str_radix(&hex, 16) {
                        let color = iced::Color::from_rgb8(
                            ((rgb >> 16) & 0xFF) as u8,
                            ((rgb >> 8) & 0xFF) as u8,
                            (rgb & 0xFF) as u8,
                        );
                        match which {
                            0 => self.glow.blue = color,
                            _ => self.glow.amber = color,
                        }
                        self.glow.invalidate();
                        self.save_config();
                    }
                }
            }
            Message::GlowReset => {
                self.glow.reset_to_defaults();
                self.glow.invalidate();
                self.glow_hex_top.clear();
                self.glow_hex_bottom.clear();
                self.save_config();
            }
            Message::KeyBindListen(action) => {
                self.keybind_capture = Some(action);
            }
            Message::KeyBindSet(action, combo) => {
                if let Some(slot) = self.keybinds.iter_mut().find(|(a, _)| *a == action) {
                    slot.1 = combo;
                }
                self.keybind_capture = None;
                self.save_config();
            }
            Message::KeyBindCancel => {
                self.keybind_capture = None;
            }
            Message::TermTabRightClick(idx) => {
                if idx < self.terminals.len() {
                    // Swap animation: whatever menu is open closes with its
                    // animation, this one opens with its own.
                    if let Some((kind, pos)) = self.term_context_menu.take() {
                        self.menu_closing.context = Some((kind, pos, 0.0));
                    }
                    self.close_shell_selector();
                    self.close_actions_menu();
                    self.tab_rename = None;
                    // Anchor the menu right below the right-clicked tab —
                    // its live layout position comes from the tween tracker.
                    let pos = self
                        .tab_tweens
                        .borrow_mut()
                        .below_of(self.terminals[idx].id as u64)
                        .unwrap_or((8.0, 0.0));
                    self.term_context_menu = Some((MenuCtx::Tab(idx), pos));
                    self.term_menu_anim_t = 0.0;
                    self.term_menu_animating = true;
                }
            }
            Message::TabRenameStart(idx) => {
                if idx < self.terminals.len() {
                    let title = self.terminals[idx].display_title(idx);
                    self.term_context_menu = None;
                    self.tab_rename = Some((idx, title));
                    return iced::widget::operation::focus(
                        iced::widget::Id::new("tab-rename"),
                    );
                }
            }
            Message::TabRenameInput(value) => {
                if let Some((_, draft)) = &mut self.tab_rename {
                    *draft = value;
                }
            }
            Message::TabRenameSubmit => {
                if let Some((idx, name)) = self.tab_rename.take() {
                    if let Some(session) = self.terminals.get_mut(idx) {
                        let name = name.trim().to_string();
                        if name.is_empty() {
                            // Empty name → back to auto numbering.
                            session.renamed = false;
                        } else {
                            session.title = name;
                            session.renamed = true;
                        }
                    }
                }
            }
            Message::TermClose(idx) => {
                // Killing a running program (vim with unsaved edits, a
                // build halfway through) should not happen on a stray
                // click — ask first.
                if self.close_confirm.is_none()
                    && self.terminals.get(idx).map(|t| t.term.is_busy()).unwrap_or(false)
                {
                    self.close_context_menu();
                    self.close_confirm = Some(CloseTarget::Tab(idx));
                    return Task::none();
                }
                self.close_confirm = None;
                if idx < self.terminals.len() {
                    self.terminals.remove(idx);
                }
                // The context menu still points at the closed tab — dismiss
                // it (animated close) so the user isn't left with a ghost
                // menu that re-closes the next tab.
                self.close_context_menu();
                self.active_terminal = if self.terminals.is_empty() {
                    None
                } else {
                    Some(idx.saturating_sub(1).min(self.terminals.len() - 1))
                };
                // Any in-flight drag is now invalid.
                self.term_tab_drag = None;
                self.tab_hover = None;
                // A stale slide would reference a closed terminal.
                self.content_anim = None;
                self.tab_rename = None;
                self.reflow_active_terminal();
                return self.sync_drag_strip();
            }
            Message::TermSelect(idx) => {
                self.select_terminal(idx);
            }
            Message::TermSelectNext => {
                if let Some(cur) = self.active_terminal {
                    let n = self.terminals.len();
                    if n > 0 {
                        self.select_terminal((cur + 1) % n);
                    }
                }
            }
            Message::TermSelectPrev => {
                if let Some(cur) = self.active_terminal {
                    let n = self.terminals.len();
                    if n > 0 {
                        self.select_terminal((cur + n - 1) % n);
                    }
                }
            }
            Message::TermCloseActive => {
                if let Some(idx) = self.active_terminal {
                    return self.update(Message::TermClose(idx));
                }
            }
            Message::TermSelectPress => {
                self.terminal_focused = true;
                self.close_context_menu();
                if self.modifiers.control() {
                    // Ctrl+左键：跳转光标下的链接 / 文件 / 位置
                    self.ctrl_click_jump();
                } else {
                    self.term_selecting = true;
                    if let Some(term) = self.active_term_mut() {
                        term.term.clear_selection();
                    }
                }
            }
            Message::TermSelectMove(x, y) => {
                self.term_mouse = (x, y);
                if self.term_selecting {
                    if let Some(term) = self.active_term_mut() {
                        let (cx, vy) = terminal::widget::pixel_to_cell(&term.term, x, y);
                        if term.term.selection.is_none() {
                            term.term.start_selection(cx, vy);
                        } else {
                            term.term.extend_selection(cx, vy);
                        }
                    }
                }
            }
            Message::TermSelectRelease => {
                self.term_selecting = false;
            }
            Message::TermTabPress(idx) => {
                // Start a potential drag; becomes a real drag only once the
                // cursor moves across another tab (see TermTabHover).
                self.term_tab_drag = Some(TabDrag { idx, reordered: false });
                self.tab_hover = Some(idx);
                // Play the lift-in animation.
                self.tab_drag_t = 0.0;
            }
            Message::TermTabHover(idx, _x, _y) => {
                self.tab_hover = Some(idx);
                // Live reorder while dragging: crossing another tab moves
                // the dragged tab to that position immediately.
                if let Some(drag) = self.term_tab_drag {
                    if drag.idx != idx {
                        self.reorder_tab(drag.idx, idx);
                    }
                }
            }
            Message::TermTabExit(idx) => {
                if self.tab_hover == Some(idx) {
                    self.tab_hover = None;
                }
            }
            Message::TermTabRelease => {
                if let Some(drag) = self.term_tab_drag.take() {
                    // A real drag ends with a settle pulse at the new slot;
                    // a plain click (no reorder) selects the tab.
                    if drag.reordered {
                        self.tab_settle = Some((drag.idx, 0.0));
                    } else {
                        self.select_terminal(drag.idx);
                    }
                }
                self.tab_drag_t = 0.0;
            }
            Message::QuickToggle => {
                // Launch the standalone quick terminal. If it is already
                // running, the new process signals it to exit (toggle).
                // stderr is inherited so the [quick] debug logs surface in
                // whatever terminal launched the main app.
                let exe = std::env::current_exe().unwrap_or_default();
                let _ = std::process::Command::new(exe).arg("--quick").spawn();
            }
            Message::RightClickTerminal => {
                // Open the context menu at the last known mouse position,
                // clamped so the menu stays inside the window.
                let (ww, wh) = self.window_size;
                let (x, y) = self.term_mouse;
                let x = x.clamp(0.0, (ww - 30.0 - 188.0).max(0.0));
                let y = y.clamp(0.0, (wh - 60.0 - 158.0).max(0.0));
                // Right-clicking elsewhere while a menu is open: the old one
                // plays its close animation at its old position, the new one
                // plays its open animation at the new position.
                if let Some((kind, old)) = self.term_context_menu.take() {
                    self.menu_closing.context = Some((kind, old, 0.0));
                }
                self.tab_rename = None;
                self.term_context_menu = Some((MenuCtx::Terminal, (x, y)));
                self.term_menu_anim_t = 0.0;
                self.term_menu_animating = true;
                self.close_shell_selector();
                self.close_actions_menu();
            }
            Message::TermClear => {
                self.close_actions_menu();
                self.close_context_menu();
                if let Some(term) = self.active_term_mut() {
                    // 1. Ask the shell to clear and redraw its prompt
                    //    (readline's Ctrl+L). The shell repositions its own
                    //    cursor, so the prompt reappears at the top.
                    term.term.input(b"\x0c");
                    // 2. Locally wipe scrollback + visible screen and reset
                    //    the cursor to the top-left, so anything the shell
                    //    emits before its redraw still lands at the top
                    //    instead of the stale mid-screen cursor position
                    //    (the old erase_screen() bug).
                    let buf = &mut term.term.buf;
                    let attr = buf.cur_attr;
                    buf.lines.clear();
                    for _ in 0..buf.rows {
                        let mut line = terminal::buffer::BufferLine::new(buf.cols);
                        line.clear(attr);
                        buf.lines.push(line);
                    }
                    buf.cursor_x = 0;
                    buf.cursor_y = 0;
                    term.term.clear_selection();
                }
            }
            Message::TermToggleStatusbar => {
                self.term_statusbar_visible = !self.term_statusbar_visible;
                self.toggle_anim_target[0] = self.term_statusbar_visible as usize;
                self.reflow_active_terminal();
                self.close_actions_menu();
                self.close_context_menu();
                self.save_config();
            }
            Message::TermToggleTabsVertical => {
                self.term_tabs_vertical = !self.term_tabs_vertical;
                self.toggle_anim_target[1] = self.term_tabs_vertical as usize;
                self.reflow_active_terminal();
                self.close_actions_menu();
                self.close_context_menu();
                self.save_config();
                // Tabs that never fit the horizontal strip were never
                // laid out, so the FLIP tracker has no position for them.
                // Seed one at the strip's right edge so they glide in
                // with the rest instead of popping into the sidebar.
                if self.term_tabs_vertical {
                    let (ww, _) = self.window_size;
                    let mut tracker = self.tab_tweens.borrow_mut();
                    for term in &self.terminals {
                        tracker.seed_missing(term.id as u64, (ww - 60.0, 6.0));
                    }
                }
                // The freshly mounted scrollable has no state — re-assert
                // the remembered offset for the new layout after it has
                // been built (one frame later).
                return Task::future(async {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Message::RestoreTabsScroll
                });
            }
            Message::RestoreTabsScroll => {
                use iced::widget::scrollable::AbsoluteOffset;
                if self.term_tabs_vertical {
                    if self.sidebar_scroll_y > 0.0 {
                        return iced::widget::operation::scroll_to(
                            iced::widget::Id::new("sidebar-tabs"),
                            AbsoluteOffset {
                                x: None,
                                y: Some(self.sidebar_scroll_y),
                            },
                        );
                    }
                } else if self.tabs_scroll_x > 0.0 {
                    return iced::widget::operation::scroll_to(
                        iced::widget::Id::new("titlebar-tabs"),
                        AbsoluteOffset {
                            x: Some(self.tabs_scroll_x),
                            y: None,
                        },
                    );
                }
            }
            Message::TermShowShellSelector => {
                // Press again does NOT close — clicking outside or picking
                // an item does (matches VS Code menu semantics).
                if !self.term_shell_selector_open {
                    self.close_actions_menu();
                    self.close_context_menu();
                    self.term_shell_selector_open = true;
                    self.term_menu_anim_t = 0.0;
                    self.term_menu_animating = true;
                }
            }
            Message::TermShowActionsMenu => {
                if !self.term_actions_menu_open {
                    self.close_shell_selector();
                    self.close_context_menu();
                    self.term_actions_menu_open = true;
                    self.term_menu_anim_t = 0.0;
                    self.term_menu_animating = true;
                }
            }
            Message::TermCloseMenus => {
                self.close_shell_selector();
                self.close_actions_menu();
                self.close_context_menu();
                self.tab_rename = None;
            }
            Message::TermCloseAll => {
                self.terminals.clear();
                self.active_terminal = None;
                self.close_actions_menu();
                self.close_context_menu();
                self.tab_rename = None;
                return self.sync_drag_strip();
            }
            Message::TermWrite(bytes) => {
                // While the search bar is open, Esc closes it instead of
                // being sent to the shell.
                if self.search_open && bytes == b"\x1b" {
                    return self.update(Message::SearchClose);
                }
                if let Some(term) = self.active_term_mut() {
                    term.term.input(&bytes);
                }
                self.terminal_focused = true;
            }
            Message::TermResize(cols, rows, w, h) => {
                // Ignore spurious resize events when the dimensions haven't
                // actually changed — the sensor fires on every view() even
                // when the pixel size is identical.
                if self.term_content_size == (w, h) {
                    return Task::none();
                }
                self.term_content_size = (w, h);
                if let Some(term) = self.active_term_mut() {
                    if term.term.buf.cols != cols
                        || term.term.buf.rows != rows
                    {
                        term.term.resize(cols, rows);
                    }
                }
                if !self.term_pty_resize_pending {
                    self.term_pty_resize_pending = true;
                    return Task::future(async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                        Message::TermFlushPtyResize
                    });
                }
            }
            Message::TermFlushPtyResize => {
                self.term_pty_resize_pending = false;
                // Only flush terminals that actually have a pending resize.
                for term in &mut self.terminals {
                    if term.term.has_pending_pty_resize() {
                        term.term.flush_pty_resize();
                    }
                }
            }
            Message::TermCopy => {
                self.close_context_menu();
                if let Some(term) = self.active_term() {
                    let text = term.term.selected_text();
                    if !text.is_empty() {
                        return iced::clipboard::write(text);
                    }
                }
            }
            Message::TermPaste => {
                self.close_context_menu();
                return iced::clipboard::read().map(|s| {
                    if let Some(text) = s {
                        Message::TermWrite(text.into_bytes())
                    } else {
                        // No text on the clipboard — do nothing.
                        Message::Nop
                    }
                });
            }
            Message::TermSelectAll => {
                self.close_context_menu();
                if let Some(term) = self.active_term_mut() {
                    term.term.select_all();
                }
            }
            Message::ShellIntegrationEnable => {
                let shell = resolve_shell_path(&self.term_default_shell);
                self.shell_notice =
                    match crate::shell_integration::enable(&shell) {
                        Ok(()) => Some("已启用 · 新开的标签页生效".into()),
                        Err(e) => Some(format!("启用失败：{e}")),
                    };
                self.shell_status = crate::shell_integration::status(&shell);
            }
            Message::ShellIntegrationDisable => {
                let shell = resolve_shell_path(&self.term_default_shell);
                self.shell_notice = match crate::shell_integration::disable(&shell) {
                    Ok(()) => Some("已移除 · 新开的标签页生效".into()),
                    Err(e) => Some(format!("移除失败：{e}")),
                };
                self.shell_status = crate::shell_integration::status(&shell);
            }
            Message::ShellCopyInstallCmd => {
                let cmd = self.shell_status.install_command();
                if !cmd.is_empty() {
                    return iced::clipboard::write(cmd);
                }
            }
            Message::ShellPasteInstallCmd => {
                let cmd = self.shell_status.install_command();
                if !cmd.is_empty() {
                    let mut text = cmd;
                    text.push(' ');
                    if let Some(term) = self.active_term_mut() {
                        term.term.input(text.as_bytes());
                    }
                    self.terminal_focused = true;
                }
            }
            Message::FileDropped(path) => {
                // A file/folder dragged in from the file manager inserts
                // its shell-quoted path, the way every other terminal
                // does. Only into a live shell — never into the
                // settings page, and never into a dead PTY.
                let ready = self
                    .active_term_mut()
                    .map(|t| t.kind == SessionKind::Terminal && t.term.is_alive())
                    .unwrap_or(false);
                if ready {
                    let mut text = terminal::shell_quote_path(&path);
                    text.push(' ');
                    if let Some(term) = self.active_term_mut() {
                        term.term.input(text.as_bytes());
                    }
                    self.terminal_focused = true;
                    self.close_context_menu();
                }
            }
            Message::ToggleTerminal => {}
            Message::CloseWindow => {
                if self.close_confirm.is_none() && self.active_term_busy() {
                    self.close_confirm = Some(CloseTarget::Window);
                    return Task::none();
                }
                self.close_confirm = None;
                if let Some(id) = self.main_window {
                    return iced::window::close(id);
                }
            }
            Message::CloseConfirmCancel => {
                self.close_confirm = None;
            }
            Message::CloseConfirmAccept => {
                let target = self.close_confirm.take();
                match target {
                    Some(CloseTarget::Window) => {
                        if let Some(id) = self.main_window {
                            return iced::window::close(id);
                        }
                    }
                    Some(CloseTarget::Tab(idx)) => {
                        // Force the close even if the tab still looks
                        // busy — the user just confirmed it.
                        self.close_confirm = None;
                        return self.update(Message::TermClose(idx));
                    }
                    None => {}
                }
            }
            Message::MinimizeWindow => {
                if let Some(id) = self.main_window {
                    return iced::window::minimize(id, true);
                }
            }
            Message::ToggleMaximize => {
                if let Some(id) = self.main_window {
                    return iced::window::toggle_maximize(id);
                }
            }
            Message::DragTitlebar => {
                if let Some(id) = self.main_window {
                    return iced::window::drag(id);
                }
            }
            Message::MainWindowReady(id) => {
                self.main_window = Some(id);
            }
            Message::Tick => {
                // Advance cursor animations and blink
                let dt_ms = 500.0;
                for session in &mut self.terminals {
                    session.term.advance_cursor_anim(dt_ms);
                    session.term.tick_blink(self.terminal_focused);
                }
            }
            Message::Nop => {}
            Message::TermPump => {
                // Drain PTY output for all live terminals
                for session in &mut self.terminals {
                    session.term.pump();
                }
                // Close tabs whose shell has exited — a dead PTY would
                // otherwise silently swallow every keystroke (write errors
                // are intentionally ignored by the terminal core). Reuse
                // `TermClose` so active-tab fixups and the drag-strip
                // sync all apply; it is bounds-safe.
                let mut dead = Vec::new();
                for (idx, session) in self.terminals.iter_mut().enumerate() {
                    // Settings (and any other pseudo-session) has no PTY —
                    // is_alive() is always false. Don't auto-close it.
                    if session.kind == SessionKind::Settings {
                        continue;
                    }
                    if !session.term.is_alive() {
                        dead.push(idx);
                    }
                }
                // Highest index first so removals don't shift pending ones.
                let mut tasks = Vec::new();
                for idx in dead.into_iter().rev() {
                    tasks.push(self.update(Message::TermClose(idx)));
                }
                return Task::batch(tasks);
            }
            Message::AnimTick => {
                // 60 fps animation tick — advance cursor animations and
                // check for measured character width changes.
                let dt_ms = 16.67; // ~60 fps
                // Advance the dropdown expand animation.
                if self.term_menu_animating {
                    self.term_menu_anim_t += dt_ms;
                    if self.term_menu_anim_t >= crate::animation::MENU_ANIM_MS {
                        self.term_menu_anim_t = crate::animation::MENU_ANIM_MS;
                        self.term_menu_animating = false;
                    }
                }
                // Advance the terminal content switch slide.
                if let Some((_, _, t)) = &mut self.content_anim {
                    *t += dt_ms;
                }
                if self
                    .content_anim
                    .is_some_and(|(_, _, t)| t >= CONTENT_SWITCH_MS)
                {
                    self.content_anim = None;
                }
                // Settings page: entrance slide + iOS toggle progress.
                let settings_front = self
                    .active_terminal
                    .and_then(|i| self.terminals.get(i))
                    .is_some_and(|s| s.kind == SessionKind::Settings);
                if settings_front {
                    if self.settings_anim_t < crate::animation::MENU_ANIM_MS {
                        self.settings_anim_t = (self.settings_anim_t + dt_ms)
                            .min(crate::animation::MENU_ANIM_MS);
                    }
                    if self.picker.is_some()
                        && self.picker_anim_t < crate::animation::MENU_ANIM_MS
                    {
                        self.picker_anim_t = (self.picker_anim_t + dt_ms)
                            .min(crate::animation::MENU_ANIM_MS);
                    }
                    for i in 0..2 {
                        let t = &mut self.toggle_progress[i];
                        let target = self.toggle_anim_target[i] as f32;
                        *t += (target - *t) * 0.25;
                        if (*t - target).abs() < 0.005 {
                            *t = target;
                        }
                    }
                }
                // Advance the search bar expand/collapse animation.
                if self.search_closing {
                    self.search_anim_t += dt_ms;
                    if self.search_anim_t >= crate::animation::MENU_CLOSE_MS {
                        self.search_open = false;
                        self.search_closing = false;
                        self.search_anim_t = 0.0;
                    }
                } else if self.search_open && self.search_anim_t < crate::animation::MENU_ANIM_MS {
                    self.search_anim_t =
                        (self.search_anim_t + dt_ms).min(crate::animation::MENU_ANIM_MS);
                }
                // Advance the dropdown close animations.
                for slot in [&mut self.menu_closing.shell, &mut self.menu_closing.actions] {
                    if let Some(t) = slot {
                        *t += dt_ms;
                        if *t >= crate::animation::MENU_CLOSE_MS {
                            *slot = None;
                        }
                    }
                }
                if let Some((_, _, t)) = &mut self.menu_closing.context {
                    *t += dt_ms;
                }
                if self
                    .menu_closing
                    .context
                    .is_some_and(|(_, _, t)| t >= crate::animation::MENU_CLOSE_MS)
                {
                    self.menu_closing.context = None;
                }
                // Drag strip: show delay → smooth window growth →
                // hide delay → smooth shrink.
                if !self.strip_visible
                    && self.strip_show_at
                        .is_some_and(|t| t.elapsed().as_millis() as f32 >= STRIP_SHOW_DELAY_MS)
                {
                    self.strip_visible = true;
                    self.strip_show_at = None;
                }
                if let Some(t0) = self.strip_hide_at {
                    if t0.elapsed().as_millis() as f32 >= STRIP_HIDE_DELAY_MS {
                        self.strip_visible = false;
                        self.strip_hide_at = None;
                    }
                }
                let strip_target =
                    if self.strip_visible && self.tabs_overflow { 1.0 } else { 0.0 };
                let speed = dt_ms / STRIP_GROW_MS;
                if self.strip_anim_t < strip_target {
                    self.strip_anim_t = (self.strip_anim_t + speed).min(1.0);
                } else if self.strip_anim_t > strip_target {
                    self.strip_anim_t = (self.strip_anim_t - speed).max(0.0);
                }
                // Drive the window height toward the animated extra. A new
                // resize request is only sent when the target moved ≥1px,
                // so the compositor isn't flooded when idle.
                let extra = STRIP_ZONE_H * crate::animation::menu_ease(self.strip_anim_t);
                let mut strip_task = Task::none();
                if (extra - self.strip_extra_applied).abs() >= 1.0 {
                    if let Some(id) = self.main_window {
                        let (w, _) = self.window_size;
                        strip_task = iced::window::resize(
                            id,
                            iced::Size::new(w, self.strip_base_h + extra),
                        );
                    }
                    self.strip_extra_applied = extra;
                }
                // Advance tab drag lift-in / drop settle animations.
                if self.term_tab_drag.is_some() {
                    self.tab_drag_t = (self.tab_drag_t + dt_ms).min(TAB_LIFT_MS);
                }
                if let Some((_, t)) = &mut self.tab_settle {
                    *t += dt_ms;
                }
                if self.tab_settle.is_some_and(|(_, t)| t >= TAB_SETTLE_MS) {
                    self.tab_settle = None;
                }
                for session in &mut self.terminals {
                    session.term.advance_cursor_anim(dt_ms);
                    session.term.tick_blink(self.terminal_focused);
                }
                // The terminal canvas overlay measures the real monospace
                // character width on its first draw. Until that happens the
                // grid is sized with a rough estimate, so the text falls a
                // couple of columns short of the right edge. Once the real
                // width arrives, reflow so the grid picks up the correct
                // column count. This only fires when the value actually
                // changes, so it's a no-op on every subsequent frame.
                // CRITICAL: Do NOT call reflow_active_terminal() here — it
                // calls flush_pty_resize() which is synchronous and blocks
                // the UI thread. Instead, just track the width change and
                // let the next natural resize event handle the PTY resize.
                if let Some(w) = terminal::widget::measured_char_width() {
                    if (w - self.term_last_char_w).abs() > 0.01 {
                        self.term_last_char_w = w;
                        // The grid was sized with the estimate — reflow it
                        // NOW with the real cell width instead of waiting
                        // for a resize event that may never come. The PTY
                        // resize stays async via the 150ms debounce task.
                        let (cols, rows) = compute_term_grid(self);
                        for session in &mut self.terminals {
                            if session.term.buf.cols != cols
                                || session.term.buf.rows != rows
                            {
                                session.term.resize(cols, rows);
                            }
                        }
                        if !self.term_pty_resize_pending {
                            self.term_pty_resize_pending = true;
                            return Task::batch([
                                strip_task,
                                Task::future(async {
                                    tokio::time::sleep(
                                        std::time::Duration::from_millis(150),
                                    )
                                    .await;
                                    Message::TermFlushPtyResize
                                }),
                            ]);
                        }
                    }
                }
                return strip_task;
            }
            Message::ImeCommit(content) => {
                // IME committed text (e.g. Chinese from fcitx5/ibus) —
                // feed it to the focused terminal.
                return self.update(Message::TermWrite(content.into_bytes()));
            }
            Message::ImePreedit(content) => {
                self.ime_preedit = content;
            }
            Message::Event(event) => {
                return self.handle_event(event);
            }
        }
        Task::none()
    }

    fn active_term_mut(&mut self) -> Option<&mut TerminalSession> {
        let idx = self.active_terminal?;
        self.terminals.get_mut(idx)
    }

    /// Persist the current user preferences to disk.
    fn save_config(&self) {
        crate::config::save(&crate::config::Config {
            statusbar_visible: self.term_statusbar_visible,
            tabs_vertical: self.term_tabs_vertical,
            sidebar_width: self.term_tab_width,
            shell: self.term_default_shell.clone(),
            glow_blue: crate::glow::rgb_of(self.glow.blue),
            glow_amber: crate::glow::rgb_of(self.glow.amber),
            glow_intensity_top: self.glow.intensity_top,
            glow_intensity_bottom: self.glow.intensity_bottom,
            keybinds: self
                .keybinds
                .iter()
                .map(|(a, c)| (a.id().to_string(), c.clone()))
                .collect(),
        });
    }

    fn active_term(&self) -> Option<&TerminalSession> {
        let idx = self.active_terminal?;
        self.terminals.get(idx)
    }

    /// Whether the settings session is in front (drives entrance/toggle
    /// animations, which only need ticks while visible).
    fn settings_front_anim(&self) -> bool {
        self.active_terminal
            .and_then(|i| self.terminals.get(i))
            .is_some_and(|s| s.kind == SessionKind::Settings)
    }

    /// Select a terminal with the content slide animation. Returns true if
    /// the selection actually changed.
    fn select_terminal(&mut self, idx: usize) -> bool {
        if idx >= self.terminals.len() {
            return false;
        }
        if self.active_terminal == Some(idx) {
            return false;
        }
        let from = self.active_terminal.unwrap_or(idx);
        // Slide direction derives from tab order: target on the right of
        // the current tab → content slides left (new comes from right).
        self.content_anim = Some((from, idx, 0.0));
        self.active_terminal = Some(idx);
        self.reflow_active_terminal();
        true
    }

    /// The terminal content area (with the switch-slide animation).
    fn terminal_body(&self) -> iced::Element<'_, Message> {
        match self.active_terminal {
            Some(idx) if idx < self.terminals.len() => {
                // Content switch slide: outgoing and incoming terminal views
                // translate side by side, direction derived from tab order.
                if let Some((from, to, t)) = self.content_anim {
                    let (from, to) = if from < self.terminals.len() {
                        (from, to)
                    } else {
                        (to, to)
                    };
                    let p = crate::animation::ease_out_cubic(
                        (t / CONTENT_SWITCH_MS).clamp(0.0, 1.0),
                    );
                    let dir = if to > from { 1.0 } else { -1.0 };
                    // Vertical tabs → the content slides vertically (target
                    // below → comes from below); otherwise horizontally.
                    let (ox, oy, ix, iy) = if self.term_tabs_vertical {
                        let h = self.term_content_size.1.max(1.0);
                        (0.0, -dir * h * p, 0.0, dir * h * (1.0 - p))
                    } else {
                        let w = self.term_content_size.0.max(1.0);
                        (-dir * w * p, 0.0, dir * w * (1.0 - p), 0.0)
                    };
                    let focused = self.terminal_focused;
                    let mk_view = |i: usize, is_focused: bool| {
                        terminal::widget::terminal_view(
                            &self.terminals[i].term,
                            is_focused,
                            iced::widget::Id::from(format!("terminal-scroll-{i}")),
                            |cols, rows, w, h| {
                                Message::TermResize(cols, rows, w, h)
                            },
                        )
                    };
                    let outgoing = crate::animation::Shifted::new(
                        iced::Vector::new(ox, oy),
                        mk_view(from, false),
                    );
                    let incoming = crate::animation::Shifted::new(
                        iced::Vector::new(ix, iy),
                        mk_view(to, focused),
                    );
                    stack![
                        container(outgoing).width(Length::Fill).height(Length::Fill).clip(true),
                        container(incoming).width(Length::Fill).height(Length::Fill).clip(true),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
                } else {
                    let term = &self.terminals[idx];
                    terminal::widget::terminal_view(
                        &term.term,
                        self.terminal_focused,
                        iced::widget::Id::from(format!("terminal-scroll-{idx}")),
                        Message::TermResize,
                    )
                }
            }
            _ => empty_state(),
        }
    }

    /// Rough estimate of whether the tab strip overflows the titlebar.
    /// Tab pill width ≈ 24px padding + 12px icon + 6px gap + title chars
    /// (7px avg at 12px font) + 6px gap + 14px close + 4px spacing.
    fn tabs_overflow_est(&self) -> bool {
        if self.term_tabs_vertical || self.terminals.len() < 2 {
            return false;
        }
        // Use the real measured monospace cell width when available instead
        // of a hardcoded 7px guess; wide (CJK) chars count as two cells.
        let cw = if self.term_last_char_w > 0.5 {
            self.term_last_char_w
        } else {
            7.0
        };
        let est: f32 = self
            .terminals
            .iter()
            .enumerate()
            .map(|(idx, t)| 62.0 + title_cell_width(&t.display_title(idx)) as f32 * cw)
            .sum();
        let (ww, _) = self.window_size;
        // titlebar padding 2*12 + left group (~90) + right buttons (~132)
        let avail = (ww - 24.0 - 90.0 - 132.0).max(0.0);
        est > avail
    }

    /// Whether the cursor is over the tab-strip zone of the titlebar —
    /// the middle stretch, excluding the traffic lights (left) and the
    /// action buttons (right) so the strip won't pop up over them.
    fn titlebar_over_tabs(&self) -> bool {
        self.titlebar_hover_x.is_some_and(|x| {
            let (ww, _) = self.window_size;
            x >= 100.0 && x <= ww - 160.0
        })
    }

    /// Sync the overflow flag that gates the drag-strip zone. The window
    /// height itself animates in AnimTick (per-frame resize requests) once
    /// the hover delay elapses. Wayland supports resize; only moving the
    /// window is restricted, so the extra space appears at the bottom edge
    /// while the strip zone occupies the top of the layout.
    fn sync_drag_strip(&mut self) -> Task<Message> {
        let overflow = self.tabs_overflow_est();
        if overflow == self.tabs_overflow {
            return Task::none();
        }
        self.tabs_overflow = overflow;
        if overflow {
            // Remember the un-grown window height as the resize base.
            self.strip_base_h = self.window_size.1;
            // Mouse may already be over the tab region.
            if self.titlebar_over_tabs() && self.strip_show_at.is_none() {
                self.strip_show_at = Some(std::time::Instant::now());
            }
        } else {
            self.strip_visible = false;
            self.strip_show_at = None;
            self.strip_hide_at = None;
            // anim shrinks back to 0; AnimTick resizes down as it goes.
        }
        Task::none()
    }

    /// Begin (or restart) the close animation for a menu kind.
    /// Reveal/hide bookkeeping for the overflow drag strip. Called from the
    /// hover messages; the delays and animated progress advance in AnimTick.
    fn sync_strip_visibility(&mut self) {
        let hover = self.titlebar_over_tabs() || self.titlebar_grip_hover;
        if hover {
            self.strip_hide_at = None;
            if self.tabs_overflow
                && !self.strip_visible
                && self.strip_show_at.is_none()
            {
                // Extend only after the hover delay elapses.
                self.strip_show_at = Some(std::time::Instant::now());
            }
        } else {
            // Left the titlebar: cancel a pending reveal, retract after
            // the grace period if currently shown.
            self.strip_show_at = None;
            if self.strip_visible && self.strip_hide_at.is_none() {
                self.strip_hide_at = Some(std::time::Instant::now());
            }
        }
    }

    fn start_closing(&mut self, kind: MenuKind) {
        let slot = match kind {
            MenuKind::ShellSelector => &mut self.menu_closing.shell,
            MenuKind::Actions => &mut self.menu_closing.actions,
        };
        *slot = Some(0.0);
    }

    fn close_shell_selector(&mut self) {
        if self.term_shell_selector_open {
            self.term_shell_selector_open = false;
            self.start_closing(MenuKind::ShellSelector);
        }
    }

    fn close_actions_menu(&mut self) {
        if self.term_actions_menu_open {
            self.term_actions_menu_open = false;
            self.start_closing(MenuKind::Actions);
        }
    }

    fn close_context_menu(&mut self) {
        if let Some((kind, pos)) = self.term_context_menu.take() {
            self.menu_closing.context = Some((kind, pos, 0.0));
        }
    }

    /// Move a terminal tab from index `from` to index `to`, keeping the
    /// active-terminal index and drag state consistent.
    fn reorder_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.terminals.len() || to >= self.terminals.len() {
            return;
        }
        let session = self.terminals.remove(from);
        self.terminals.insert(to, session);

        // Track the active terminal through the shift.
        if self.active_terminal == Some(from) {
            self.active_terminal = Some(to);
        } else if let Some(a) = self.active_terminal {
            if from < to && a > from && a <= to {
                self.active_terminal = Some(a - 1);
            } else if to < from && a >= to && a < from {
                self.active_terminal = Some(a + 1);
            }
        }

        if let Some(drag) = &mut self.term_tab_drag {
            drag.idx = to;
            drag.reordered = true;
        }
    }

    /// Ctrl+click: jump to whatever sits under the cursor — an URL, a file
    /// path, or a `file:line` location. Opens URLs with the system opener.
    fn ctrl_click_jump(&mut self) {
        let Some(idx) = self.active_terminal else {
            return;
        };
        let term = &self.terminals[idx].term;
        let (x, y) = self.term_mouse;
        let (cx, vy) = terminal::widget::pixel_to_cell(term, x, y);
        let word = word_at_cell(term, cx, vy);
        if let Some(target) = classify_jump_target(&word) {
            // Fire-and-forget; xdg-open returns immediately on Linux.
            let _ = std::process::Command::new("xdg-open")
                .arg(&target)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }

    fn reflow_active_terminal(&mut self) {
        if self.active_terminal.is_none() || self.terminals.is_empty() {
            return;
        }
        let (cols, rows) = compute_term_grid(self);
        if let Some(term) = self.active_term_mut() {
            if term.term.buf.cols != cols || term.term.buf.rows != rows {
                // resize() resizes the buffer immediately and defers the
                // PTY resize. These call-sites are discrete events
                // (status-bar toggle, tab open/close…) rather than a
                // rapid drag stream, so flush the PTY resize right away.
                term.term.resize(cols, rows);
                term.term.flush_pty_resize();
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Settings tab in front — slides up into place on entry.
        let settings_front = self
            .active_terminal
            .and_then(|i| self.terminals.get(i))
            .is_some_and(|s| s.kind == SessionKind::Settings);
        let body: iced::Element<'_, Message> = if settings_front {
            let p = crate::animation::menu_ease(
                (self.settings_anim_t / crate::animation::MENU_ANIM_MS).clamp(0.0, 1.0),
            );
            iced::Element::from(crate::animation::Shifted::new(
                iced::Vector::new(0.0, (1.0 - p) * 18.0),
                crate::settings::settings_panel(self),
            ))
        } else {
            self.terminal_body()
        };

        // Vertical tabs mode: sidebar on the right (no extra header, no duplicate buttons)
        let body: iced::Element<'_, Message> =
            if self.term_tabs_vertical && !self.terminals.is_empty() {
                let mut tabs_col = column![].spacing(2.0).padding([4.0, 4.0]);
                for (idx, term) in self.terminals.iter().enumerate() {
                    let active = self.active_terminal == Some(idx);
                    let hovered = self.tab_hover == Some(idx);
                    let dragging = self.term_tab_drag.is_some_and(|d| d.idx == idx);
                    let lift = if dragging { tab_lift(self) } else { 0.0 };
                    let pulse = tab_pulse(self, idx);
                    tabs_col = tabs_col.push(term_tab_vertical(
                        term.id,
                        term.display_title(idx),
                        active,
                        idx,
                        hovered,
                        lift,
                        pulse,
                        &self.tab_tweens,
                    ));
                }
                let tabs_scroll = scrollable(tabs_col)
                    .id(iced::widget::Id::new("sidebar-tabs"))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::new().width(4.0).scroller_width(4.0),
                    ))
                    .on_scroll(|vp| {
                        Message::SidebarScrolled(vp.absolute_offset().y)
                    })
                    .style(|_t, _s| styles::scroll_style());

                let tabs_panel = container(tabs_scroll)
                    .width(Pixels(self.term_tab_width))
                    .height(Length::Fill)
                    .style(|_t| styles::island())
                    .clip(true);

                row![body, resize_handle(), tabs_panel]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                body
            };

        // Mouse interaction for selection and focus
        let ime_cursor = self.active_term().and_then(|t| {
            let (cf, rf) = t.term.cursor_render_pos()?;
            let cw = terminal::widget::cell_width();
            let chh = terminal::widget::cell_height();
            // Row `rf` is painted at PAD_Y + rf * cell_height from the
            // canvas top — same origin the renderer uses. Deriving the
            // origin from the panel height (bottom-anchored) put the IME
            // caret up to a full row off, and dragged the whole click
            // mapping with it.
            Some(iced::Rectangle {
                x: terminal::widget::PAD_X + cf * cw,
                y: terminal::widget::PAD_Y + rf * chh,
                width: cw,
                height: chh,
            })
        });
        let ime_on = !self.modifiers.control();
        let ime_body = crate::animation::ImeZone::new(body)
            .ime(ime_on, self.ime_preedit.clone(), ime_cursor);
        // Search card floats at the bottom-right, expanding/collapsing
        // with the shared bezier ease; layered above the terminal but
        // inside the mouse_area so the IME zone still wraps it.
        let body_inner: iced::Element<'_, Message> = if self.search_open || self.search_closing {
            let card = container(
                animated_menu(search_bar(self), 420.0, 44.0, self.search_anim_t, false, self.search_closing),
            )
            .padding(styles::pad4(0.0, 14.0, 10.0, 0.0))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom);
            iced::widget::stack![ime_body, card]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            ime_body.into()
        };
        let body_rc = mouse_area(body_inner)
            .on_press(Message::TermSelectPress)
            .on_move(|p| Message::TermSelectMove(p.x, p.y))
            .on_release(Message::TermSelectRelease)
            .on_right_press(Message::RightClickTerminal)
            .interaction(iced::mouse::Interaction::Text);

        let mut layered =
            stack![container(body_rc).width(Length::Fill).height(Length::Fill)]
                .width(Length::Fill)
                .height(Length::Fill);

        // Menu closer (also active while a close animation plays so stray
        // clicks during the fade don't fall through to the terminal)
        if self.term_shell_selector_open
            || self.term_actions_menu_open
            || self.term_context_menu.is_some()
            || self.menu_closing.any()
            || self.tab_rename.is_some()
        {
            let closer = mouse_area(container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill))
                .on_press(Message::TermCloseMenus);
            layered = layered.push(closer);
        }

        // Shell selector dropdown (expands from just below its button)
        if self.term_shell_selector_open {
            let t = if self.term_menu_animating {
                self.term_menu_anim_t
            } else {
                crate::animation::MENU_ANIM_MS
            };
            layered = layered.push(
                container(animated_menu(shell_selector_panel(), 200.0, 142.0, t, true, false))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding([2.0, 8.0])
                    .align_x(Horizontal::Right)
                    .align_y(Vertical::Top),
            );
        } else if let Some(t) = self.menu_closing.shell {
            layered = layered.push(
                container(animated_menu(shell_selector_panel(), 200.0, 142.0, t, true, true))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding([2.0, 8.0])
                    .align_x(Horizontal::Right)
                    .align_y(Vertical::Top),
            );
        }

        // Actions menu dropdown (expands from just below its button)
        if self.term_actions_menu_open {
            let t = if self.term_menu_animating {
                self.term_menu_anim_t
            } else {
                crate::animation::MENU_ANIM_MS
            };
            layered = layered.push(
                container(animated_menu(actions_menu_panel(self), 200.0, 126.0, t, true, false))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding([2.0, 8.0])
                    .align_x(Horizontal::Right)
                    .align_y(Vertical::Top),
            );
        } else if let Some(t) = self.menu_closing.actions {
            layered = layered.push(
                container(animated_menu(actions_menu_panel(self), 200.0, 126.0, t, true, true))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding([2.0, 8.0])
                    .align_x(Horizontal::Right)
                    .align_y(Vertical::Top),
            );
        }

        // Terminal / tab right-click context menu — the closing copy renders
        // at its OLD position while the reopened one plays its open animation
        // at the new right-click position (both visible = smooth swap).
        if let Some((kind, pos, t)) = self.menu_closing.context {
            let (mw, mh) = context_menu_size(kind);
            layered = layered.push(
                container(animated_menu(context_menu_panel(self, kind), mw, mh, t, false, true))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(iced::Padding {
                        top: pos.1,
                        right: 0.0,
                        bottom: 0.0,
                        left: pos.0,
                    })
                    .align_x(Horizontal::Left)
                    .align_y(Vertical::Top),
            );
        }
        if let Some((kind, pos)) = &self.term_context_menu {
            let t = if self.term_menu_animating {
                self.term_menu_anim_t
            } else {
                crate::animation::MENU_ANIM_MS
            };
            let (mw, mh) = context_menu_size(*kind);
            layered = layered.push(
                container(animated_menu(context_menu_panel(self, *kind), mw, mh, t, false, false))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(iced::Padding {
                        top: pos.1,
                        right: 0.0,
                        bottom: 0.0,
                        left: pos.0,
                    })
                    .align_x(Horizontal::Left)
                    .align_y(Vertical::Top),
            );
        }
        // Tab rename editor (opened by right-clicking a tab)
        if let Some((idx, draft)) = &self.tab_rename {
            let _ = idx;
            let draft = draft.clone();
            layered = layered.push(
                container(
                    container(
                        iced::widget::text_input("标签名称", &draft)
                            .id(iced::widget::Id::new("tab-rename"))
                            .on_input(Message::TabRenameInput)
                            .on_submit(Message::TabRenameSubmit)
                            .size(12.0)
                            .width(Length::Fill),
                    )
                    .width(Pixels(200.0))
                    .padding([6.0, 8.0])
                    .style(|_t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(theme::BG_PRIMARY)),
                        border: iced::Border {
                            color: theme::BORDER,
                            width: 1.0,
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    }),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding([4.0, 8.0])
                .align_x(Horizontal::Right)
                .align_y(Vertical::Top),
            );
        }

        // "A program is still running" confirmation. Sits above every
        // other layer, and the shield swallows clicks so nothing behind
        // it reacts while the dialog is up.
        if let Some(target) = self.close_confirm {
            let (title, body) = match target {
                CloseTarget::Window => (
                    "关闭 Korterm？",
                    "当前终端里还有程序在运行，关闭会直接终止它。",
                ),
                CloseTarget::Tab(_) => (
                    "关闭标签页？",
                    "这个终端里还有程序在运行，关闭会直接终止它。",
                ),
            };
            let panel = container(
                column![
                    text(title).size(13.0).color(theme::TEXT),
                    text(body).size(11.0).color(theme::DIM),
                    row![
                        button(text("取消").size(12.0).color(theme::TEXT))
                            .padding([6.0, 14.0])
                            .on_press(Message::CloseConfirmCancel)
                            .style(crate::styles::dialog_button(false)),
                        button(text("仍然关闭").size(12.0).color(theme::BG_PRIMARY))
                            .padding([6.0, 14.0])
                            .on_press(Message::CloseConfirmAccept)
                            .style(crate::styles::dialog_button(true)),
                    ]
                    .spacing(8.0),
                ]
                .spacing(10.0)
                .width(Length::Fill),
            )
            .width(Pixels(300.0))
            .padding([16.0, 18.0])
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::BG_PANEL)),
                border: iced::Border {
                    color: theme::BORDER,
                    width: 1.0,
                    radius: iced::border::Radius::from(12.0),
                },
                ..Default::default()
            });

            layered = layered.push(
                mouse_area(container(iced::widget::Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill))
                .on_press(Message::CloseConfirmCancel),
            );
            layered = layered.push(
                container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            );
        }

        // Assemble island (no extra header — all controls are in the titlebar)
        let mut island = column![
            layered,
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        // Status bar
        if self.term_statusbar_visible {
            let count = self.terminals.len();
            island = island.push(
                container(
                    row![
                        text("Korterm").size(11.0).color(theme::DIM),
                        iced::widget::Space::new().width(Length::Fill),
                        text(format!("{count} 个会话"))
                            .size(11.0)
                            .color(theme::DIM),
                    ]
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Pixels(26.0))
                .padding([0.0, 14.0])
                .style(|_t| iced::widget::container::Style {
                    border: iced::Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::default(),
                    },
                    ..iced::widget::container::Style::default()
                })
                .align_y(Alignment::Center),
            );
        }

        container(island)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_t| styles::island())
            .clip(true)
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // 500 ms heartbeat — cursor blink, animations.
        let tick = iced::time::every(std::time::Duration::from_millis(500))
            .map(|_| Message::Tick);

        // ~30 fps PTY pump — only while terminals exist.
        // Separated from Tick so parser work never blocks cursor blink.
        let term_pump = if self.terminals.is_empty() {
            Subscription::none()
        } else {
            iced::time::every(std::time::Duration::from_millis(33))
                .map(|_| Message::TermPump)
        };

        // ~60 fps animation tick — only while cursor animation is active
        // or the terminal is focused (for blink). This is the key
        // difference from the original: without this subscription, the
        // character width measurement never triggers a reflow, causing
        // the UI to block on the first frame when the font is finally
        // measured.
        let anim = if self.terminals.iter().any(|t| t.term.is_cursor_animating())
            || self.terminal_focused
            || self.term_last_char_w == 0.0
            || self.term_menu_animating
            || self.menu_closing.any()
            || self.term_tab_drag.is_some()
            || self.tab_settle.is_some()
            || self.content_anim.is_some()
            || self.search_closing
            || (self.search_open && self.search_anim_t < crate::animation::MENU_ANIM_MS)
            || (self.settings_front_anim()
                && (self.settings_anim_t < crate::animation::MENU_ANIM_MS
                    || self.toggle_progress[0] != self.toggle_anim_target[0] as f32
                    || self.toggle_progress[1] != self.toggle_anim_target[1] as f32
                    || (self.picker.is_some()
                        && self.picker_anim_t < crate::animation::MENU_ANIM_MS)))
            || self.strip_anim_t > 0.0
            || (self.tabs_overflow && (self.titlebar_hover_x.is_some() || self.titlebar_grip_hover))
        {
            iced::time::every(std::time::Duration::from_millis(16))
                .map(|_| Message::AnimTick)
        } else {
            Subscription::none()
        };

        // Global keyboard/mouse events
        let events = iced::event::listen_with(|event, status, _id| match (&status, &event) {
            (_, Event::Window(iced::window::Event::Resized(_size))) => {
                Some(Message::Event(event))
            }
            (_, Event::Window(iced::window::Event::FileDropped(path))) => {
                Some(Message::FileDropped(path.clone()))
            }
            (_, Event::Keyboard(k)) => Some(keyboard_event_to_message(k, status)),
            (
                _,
                Event::InputMethod(iced::advanced::input_method::Event::Commit(content)),
            ) => Some(Message::ImeCommit(content.clone())),
            (
                _,
                Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                    content, _,
                )),
            ) => Some(Message::ImePreedit(content.clone())),
            (iced::event::Status::Ignored, Event::Mouse(_)) => Some(Message::Event(event)),
            _ => None,
        });

        Subscription::batch([tick, term_pump, anim, events])
    }

    fn handle_event(&mut self, event: Event) -> Task<Message> {
        match &event {
            Event::Window(iced::window::Event::Resized(size)) => {
                self.window_size = (size.width, size.height);
                // Recompute the resize base: the compositor may have
                // adjusted our requested size. The strip zone (applied
                // extra) sits above the titlebar and hosts no content.
                self.strip_base_h = size.height - self.strip_extra_applied;
                self.terminal_height = self.strip_base_h - 44.0 - 26.0;
                // Whether the tabs overflow depends on the window width,
                // so a maximize / un-maximize changes the answer without
                // changing the tab count. Re-estimate here, otherwise the
                // drag strip gets stuck showing after a maximize (or stuck
                // hidden after leaving fullscreen) until the next tab
                // add/remove.
                let mut tasks = vec![self.sync_drag_strip()];
                // A resize re-lays out the titlebar tab strip and resets
                // its scroll offset — restore the saved position.
                if self.tabs_scroll_x > 0.0 {
                    tasks.push(iced::widget::operation::scroll_to(
                        iced::widget::Id::new("titlebar-tabs"),
                        iced::widget::scrollable::AbsoluteOffset {
                            x: Some(self.tabs_scroll_x),
                            y: None,
                        },
                    ));
                }
                return Task::batch(tasks);
            }
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                self.modifiers = *m;
            }
            Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => {
                // Scrollback browsing: wheel over the terminal scrolls the
                // viewport (up = into history).
                let dy = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => *y,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => *y / 20.0,
                };
                if dy.abs() > 0.01 {
                    if let Some(term) = self.active_term_mut() {
                        term.term.scroll_lines(-dy as i32 * 3);
                    }
                }
            }
            Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                if self.sidebar_dragging {
                    // Sidebar hugs the right edge; the handle drags its
                    // left border. Clamp: 120px min, leave ≥200px content.
                    let (ww, _) = self.window_size;
                    self.term_tab_width =
                        (ww - position.x).clamp(120.0, (ww - 200.0).max(120.0));
                }
            }
            Event::Mouse(iced::mouse::Event::ButtonReleased(_)) => {
                self.term_selecting = false;
                // Fallback: end any tab drag whose release happened outside
                // of every tab's bounds (no tab saw the release event).
                if let Some(drag) = self.term_tab_drag.take() {
                    if drag.reordered {
                        self.tab_settle = Some((drag.idx, 0.0));
                    } else {
                        self.select_terminal(drag.idx);
                    }
                }
                self.tab_drag_t = 0.0;
                self.picker_drag = 0;
                if self.sidebar_dragging {
                    self.sidebar_dragging = false;
                    self.save_config();
                }
            }
            _ => {}
        }
        Task::none()
    }
}

// =============================================================================
// Dropdown panels with rounded corners
// =============================================================================

fn shell_selector_panel() -> iced::Element<'static, Message> {
    let shells = ["bash", "zsh", "fish", "sh"];
    let mut items = column![].spacing(2.0).padding([6.0, 4.0]);
    items = items.push(text("选择默认配置文件").size(11.0).color(theme::DIM));

    for s in shells {
        items = items.push(
            button(
                row![
                    crate::icons::icon(Icon::Terminal, theme::DIM, 14.0, 2.0),
                    text(s).size(12.0).color(theme::TEXT),
                ]
                .spacing(8.0)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6.0, 10.0])
            .on_press(Message::TermNewShell(s.to_string()))
            .style(move |_t, st| match st {
                button::Status::Hovered | button::Status::Pressed => button::Style {
                    background: Some(iced::Background::Color(theme::BG_ELEVATED)),
                    ..button::Style::default()
                },
                _ => button::Style::default(),
            }),
        );
    }

    container(items)
        .width(Pixels(200.0))
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BG_PRIMARY)),
            border: iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .into()
}

fn actions_menu_panel(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let mut items = column![].spacing(2.0).padding([6.0, 4.0]);

    items = items.push(menu_item(Icon::Settings, "设置", Message::SettingsOpen));
    items = items.push(menu_item(Icon::Trash2, "清除终端", Message::TermClear));
    items = items.push(menu_item(
        if app.term_statusbar_visible { Icon::EyeOff } else { Icon::Eye },
        if app.term_statusbar_visible { "隐藏状态栏" } else { "显示状态栏" },
        Message::TermToggleStatusbar,
    ));
    items = items.push(menu_item(Icon::X, "关闭所有终端", Message::TermCloseAll));

    container(items)
        .width(Pixels(200.0))
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BG_PRIMARY)),
            border: iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .into()
}

fn menu_item(icon: Icon, label: &str, msg: Message) -> iced::Element<'static, Message> {
    button(
        row![
            crate::icons::icon(icon, theme::DIM, 14.0, 2.0),
            text(label.to_string()).size(12.0).color(theme::TEXT),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6.0, 10.0])
    .on_press(msg)
    .style(move |_t, st| match st {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(iced::Background::Color(theme::BG_ELEVATED)),
            ..button::Style::default()
        },
        _ => button::Style::default(),
    })
    .into()
}

// =============================================================================
// Tab widgets
// =============================================================================

/// Sidebar (vertical) tab. Widget builders legitimately take the whole
/// visual state as parameters, so allow the >7-arg lint here.
#[allow(clippy::too_many_arguments)]
fn term_tab_vertical(
    id: usize,
    title: String,
    active: bool,
    idx: usize,
    hovered: bool,
    lift: f32,
    pulse: f32,
    tab_tweens: &crate::animation::SharedTweenTracker,
) -> iced::Element<'static, Message> {
    let dragging = lift > 0.0;
    let mut content = row![
        crate::icons::icon(Icon::Terminal, theme::DIM, 12.0, 2.0),
        text(title).size(12.0).color(if active || dragging { theme::TEXT } else { theme::DIM }),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    content = content.push(iced::widget::Space::new().width(Length::Fill));

    content = content.push(
        button(
            container(crate::icons::icon(Icon::X, theme::DIM, 12.0, 2.0))
                .width(Pixels(14.0))
                .height(Pixels(14.0))
                .center_x(Pixels(14.0)),
        )
        .width(Pixels(14.0))
        .height(Pixels(14.0))
        .padding(0.0)
        .on_press(Message::TermClose(idx))
        .style(styles::icon_button(theme::DIM)),
    );

    // Same draggable treatment as the titlebar tabs; vertical tabs shift
    // leftward while lifted (asymmetric padding, same 32px footprint).
    // Tween: FLIP slide keyed by the tab's stable id.
    crate::animation::Tween::new(
        id as u64,
        tab_tweens,
        mouse_area(
            container(content)
                .width(Length::Fill)
                .padding(styles::pad4(
                    4.0,
                    12.0 + 2.0 * lift,
                    4.0,
                    12.0 - 2.0 * lift,
                ))
                .height(Pixels(32.0))
                .align_y(Alignment::Center)
                .style(move |_t| styles::pill_tab_container(active, hovered, lift, pulse)),
        )
        .on_press(Message::TermTabPress(idx))
        .on_move(move |p| Message::TermTabHover(idx, p.x, p.y))
        .on_exit(Message::TermTabExit(idx))
        .on_release(Message::TermTabRelease)
        .on_right_press(Message::TermTabRightClick(idx))
        .interaction(if dragging {
            iced::mouse::Interaction::Grabbing
        } else {
            iced::mouse::Interaction::Pointer
        }),
    )
    .into()
}

// =============================================================================
// Dropdown expand animation
// =============================================================================

/// Wraps a dropdown panel in a clip container whose size animates using a
/// cubic-bezier ease. `closing = true` plays the reverse (shrink) animation.
///
/// Both axes share the same eased clock, so X and Y always reach full size
/// on exactly the same frame; their *absolute* speeds differ according to
/// the menu's aspect ratio (the larger dimension travels faster). The panel
/// is anchored to its `anchor_right` top corner — the corner under the
/// originating button — so it grows outward/downward from the button.
fn animated_menu(
    panel: iced::Element<'static, Message>,
    target_w: f32,
    target_h: f32,
    elapsed_ms: f32,
    anchor_right: bool,
    closing: bool,
) -> iced::Element<'static, Message> {
    let e = if closing {
        // 1 → 0, accelerating into the button.
        1.0 - crate::animation::menu_close_ease(
            elapsed_ms / crate::animation::MENU_CLOSE_MS,
        )
    } else {
        // 0 → 1, exploding out of the button.
        crate::animation::menu_ease(
            elapsed_ms / crate::animation::MENU_ANIM_MS,
        )
    };
    let mut c = container(panel)
        .width(Pixels((target_w * e).max(1.0)))
        .height(Pixels((target_h * e).max(1.0)))
        .clip(true);
    c = if anchor_right {
        c.align_x(Horizontal::Right)
    } else {
        c.align_x(Horizontal::Left)
    };
    c.align_y(Vertical::Top).into()
}

// =============================================================================
// Terminal right-click context menu
// =============================================================================

/// Text-only menu item (used by the terminal context menu — no icons).
fn menu_item_text(label: &str, msg: Message) -> iced::Element<'static, Message> {
    button(
        text(label.to_string())
            .size(12.0)
            .color(theme::TEXT),
    )
    .width(Length::Fill)
    .padding([6.0, 10.0])
    .on_press(msg)
    .style(move |_t, st| match st {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(iced::Background::Color(theme::BG_ELEVATED)),
            ..button::Style::default()
        },
        _ => button::Style::default(),
    })
    .into()
}

/// Context menu natural size per kind (for the expand animation).
fn context_menu_size(kind: MenuCtx) -> (f32, f32) {
    match kind {
        MenuCtx::Terminal => (180.0, 158.0),
        MenuCtx::Tab(_) => (150.0, 66.0),
    }
}

fn context_menu_panel(
    app: &TerminalPanel,
    kind: MenuCtx,
) -> iced::Element<'static, Message> {
    let mut items = column![].spacing(2.0).padding([6.0, 4.0]);

    match kind {
        MenuCtx::Terminal => {
            items = items.push(menu_item_text("复制", Message::TermCopy));
            items = items.push(menu_item_text("粘贴", Message::TermPaste));
            items = items.push(menu_item_text("全选", Message::TermSelectAll));
            items = items.push(menu_item_text("清除终端", Message::TermClear));
            let close_msg = match app.active_terminal {
                Some(idx) => Message::TermClose(idx),
                None => Message::TermCloseMenus,
            };
            items = items.push(menu_item_text("关闭终端", close_msg));
        }
        MenuCtx::Tab(idx) => {
            items = items.push(menu_item_text("重命名", Message::TabRenameStart(idx)));
            items = items.push(menu_item_text("关闭终端", Message::TermClose(idx)));
        }
    }

    container(items)
        .width(Pixels(180.0))
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BG_PRIMARY)),
            border: iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .into()
}

// =============================================================================
// Empty state
// =============================================================================

fn empty_state() -> iced::Element<'static, Message> {
    container(
        column![
            crate::icons::icon(Icon::Terminal, theme::DIMMER, 48.0, 2.0),
            text("没有活动的终端会话").size(14.0).color(theme::DIM),
            button(text("新建终端").size(13.0).font(theme::sans()).color(theme::TEXT))
                .padding([8.0, 16.0])
                .on_press(Message::TermNew)
                .style(move |_t, s| match s {
                    button::Status::Hovered | button::Status::Pressed => button::Style {
                        background: Some(iced::Background::Color(theme::BG_ELEVATED)),
                        ..button::Style::default()
                    },
                    _ => button::Style {
                        background: Some(iced::Background::Color(theme::BG_PRIMARY)),
                        ..button::Style::default()
                    },
                }),
        ]
        .spacing(16.0)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

// =============================================================================
// Resize handle
// =============================================================================

fn resize_handle() -> iced::Element<'static, Message> {
    let grip = container(iced::widget::Space::new())
        .width(Pixels(2.0))
        .height(Length::Fill)
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(Color { a: 0.3, ..theme::BORDER })),
            ..Default::default()
        });
    mouse_area(
        container(grip)
            .width(Pixels(8.0))
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            }),
    )
    .on_press(Message::SidebarDragStart)
    .interaction(iced::mouse::Interaction::ResizingHorizontally)
    .into()
}

// =============================================================================
// Title width estimation
// =============================================================================

/// Approximate wcwidth: `true` for codepoints that occupy two cells.
fn is_wide_char(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x2_0000..=0x3_FFFD).contains(&u)
}

/// Display-cell width of a string (wide chars count as two cells).
fn title_cell_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide_char(c) { 2 } else { 1 }).sum()
}

// =============================================================================
// Ctrl+click jump: word extraction and target classification
// =============================================================================

/// Extract the whitespace-delimited "word" under the given viewport cell,
/// expanding left/right across the row (skipping wide-char continuations).
fn word_at_cell(term: &terminal::Terminal, cx: usize, vy: usize) -> String {
    let total = term.buf.lines.len();
    let offset = total.saturating_sub(term.buf.rows);
    let line_idx = offset.saturating_sub(term.view_offset) + vy;
    if line_idx >= total {
        return String::new();
    }
    let line = &term.buf.lines[line_idx];

    let is_word_char =
        |c: char| !c.is_whitespace() && !"()[]{}<>\"'`|;&$#*\\,;".contains(c);

    // Flatten the row into (cell index, char) pairs.
    let mut chars: Vec<(usize, char)> = Vec::new();
    for (i, cell) in line.cells.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        for ch in cell.chars().chars() {
            chars.push((i, ch));
        }
    }
    if chars.is_empty() {
        return String::new();
    }

    // Cell index under the cursor.
    let pos = chars.partition_point(|(i, _)| *i < cx);
    if pos >= chars.len() || !is_word_char(chars[pos].1) {
        // Allow clicking one cell right of the word's last char.
        if pos == 0 || !is_word_char(chars[pos - 1].1) {
            return String::new();
        }
    }

    let mut start = pos.min(chars.len() - 1);
    while start > 0 && is_word_char(chars[start - 1].1) {
        start -= 1;
    }
    let mut end = pos.min(chars.len() - 1);
    while end + 1 < chars.len() && is_word_char(chars[end + 1].1) {
        end += 1;
    }
    chars[start..=end].iter().map(|(_, c)| c).collect()
}

/// Expand a path: `~` → home; relative → absolute via canonicalize when
/// possible, otherwise join onto the process CWD.
fn expand_path(p: &str) -> Option<std::path::PathBuf> {
    if p.is_empty() {
        return None;
    }
    let path = if let Some(rest) = p.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        std::path::PathBuf::from(p)
    };
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(&path))
    }
}

/// Percent-decode a URL path (`%20` → space, `%E4%B8%AD` → 中). Invalid
/// escapes pass through unchanged — xdg-open can still cope with them.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Classify a terminal word as a jump target: URL, file path, or
/// `path:line(:col)` location. Returns the target to hand to `xdg-open`.
fn classify_jump_target(word: &str) -> Option<String> {    let word = word
        .trim_matches(|c: char| {
            "()[]{}<>\"'`.,;:!?".contains(c) || c == '‘' || c == '’' || c == '“' || c == '”'
        })
        .trim();
    if word.is_empty() {
        return None;
    }

    // Plain URL.
    if word.starts_with("http://") || word.starts_with("https://") {
        return Some(word.to_string());
    }
    // file:// URL → local path (percent-decoded: `%20` → space, etc.).
    if let Some(rest) = word.strip_prefix("file://") {
        return Some(percent_decode(rest));
    }

    // File path, optionally followed by :line or :line:col.
    // Try from longest to shortest path prefix.
    let mut candidates: Vec<&str> = vec![word];
    if let Some(idx) = word.rfind(':') {
        candidates.push(&word[..idx]);
        if let Some(idx2) = word[..idx].rfind(':') {
            candidates.push(&word[..idx2]);
        }
    }
    for cand in candidates {
        if cand.is_empty() {
            continue;
        }
        if let Some(abs) = expand_path(cand) {
            if abs.exists() {
                return Some(abs.to_string_lossy().into_owned());
            }
        }
    }
    None
}

// =============================================================================
// Terminal creation
// =============================================================================

/// Handoff queue for asynchronously spawned terminals. A `Terminal`
/// (PTY file descriptor plus buffer) cannot be part of the `Message`
/// enum: widgets require clone-able messages, and a PTY must never be
/// duplicated. The spawn task parks the terminal here, and `update`
/// picks it up by `seq` when the clone-safe `TerminalSpawned` message
/// arrives.
static PENDING_TERMS: std::sync::Mutex<Vec<(usize, Box<terminal::Terminal>)>> =
    std::sync::Mutex::new(Vec::new());

/// Take a spawned terminal from the handoff queue by its sequence number.
fn take_pending_term(seq: usize) -> Option<Box<terminal::Terminal>> {
    let mut q = PENDING_TERMS.lock().unwrap_or_else(|e| e.into_inner());
    let pos = q.iter().position(|(s, _)| *s == seq)?;
    Some(q.remove(pos).1)
}

fn create_terminal(app: &mut TerminalPanel, shell: Option<&str>) -> Task<Message> {
    app.terminal_seq += 1;
    // Use a sensible default grid; the real size is set by the first
    // TermResize once the content sensor measures the actual panel.
    let (cols, rows) = (80usize, 24usize);
    let shell_owned = shell.map(|s| s.to_string());
    let seq = app.terminal_seq;
    let base = match shell_owned {
        Some(ref s) => s.clone(),
        None => "shell".to_string(),
    };
    let title = format!("{base} ({seq})");

    Task::perform(
        async move {
            let term = terminal::Terminal::with_shell_async(
                cols,
                rows,
                shell_owned.as_deref(),
                None,
            )
            .await;
            // Park the terminal for `update` to pick up by `seq`.
            PENDING_TERMS
                .lock()
                .unwrap()
                .push((seq, Box::new(term)));
        },
        move |_| {
            Message::TerminalSpawned { seq, title: title.clone(), base: base.clone() }
        },
    )
}

fn compute_term_grid(app: &TerminalPanel) -> (usize, usize) {
    let (ww, _wh) = app.window_size;
    let term_tabs = if app.term_tabs_vertical && !app.terminals.is_empty() {
        app.term_tab_width + 8.0
    } else {
        0.0
    };
    let panel_w = (ww - 30.0 - term_tabs).max(80.0);
    let statusbar = if app.term_statusbar_visible { 26.0 } else { 0.0 };
    let panel_h = (app.terminal_height - 2.0 - 38.0 - statusbar).max(40.0);
    terminal::widget::grid_size_for_pixels(panel_w, panel_h)
}

// =============================================================================
// Terminal search bar (Ctrl+F)
// =============================================================================

/// Compact floating search card (bottom-right). Content only — the
/// expand/collapse animation wraps this via `animated_menu` in `view`.
fn search_bar(app: &TerminalPanel) -> iced::Element<'static, Message> {
    use iced::widget::text_input;

    let (cur, total) = app
        .active_term()
        .and_then(|t| t.term.search.as_ref())
        .map(|s| (s.matches.len().min(s.current + 1), s.matches.len()))
        .unwrap_or((0, 0));

    // Current match opens a file? → offer the jump button.
    let target = app
        .active_term()
        .and_then(|t| t.term.search_current().map(|m| m.text.clone()))
        .and_then(|t| classify_jump_target(&t));

    let query = app.search_query.clone();
    let mut row = iced::widget::row![
        crate::icons::icon(Icon::Search, theme::DIM, 12.0, 2.0),
        text_input("搜索…", &query)
            .id(iced::widget::Id::new("term-search"))
            .size(12.0)
            .width(iced::Pixels(190.0))
            .on_input(Message::SearchInput)
            .on_submit(Message::SearchNav(true)),
        text(format!("{cur}/{total}")).size(11.0).color(theme::DIM),
    ]
    .spacing(6.0)
    .align_y(iced::alignment::Vertical::Center);

    row = row.push(
        iced::widget::button(crate::icons::icon(Icon::ChevronUp, theme::DIM, 12.0, 2.0))
            .padding(2.0)
            .on_press(Message::SearchNav(false))
            .style(styles::icon_button(theme::DIM)),
    );
    row = row.push(
        iced::widget::button(crate::icons::icon(Icon::ChevronDown, theme::DIM, 12.0, 2.0))
            .padding(2.0)
            .on_press(Message::SearchNav(true))
            .style(styles::icon_button(theme::DIM)),
    );
    if let Some(t) = target {
        row = row.push(
            iced::widget::button(text(format!("↗ {t}")).size(11.0).color(theme::TEXT))
                .padding([2.0, 6.0])
                .on_press(Message::SearchOpenTarget)
                .style(styles::icon_button(theme::TEXT)),
        );
    }
    row = row.push(
        iced::widget::button(crate::icons::icon(Icon::X, theme::DIM, 12.0, 2.0))
            .padding(2.0)
            .on_press(Message::SearchClose)
            .style(styles::icon_button(theme::DIM)),
    );

    container(row)
        .padding([6.0, 10.0])
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BG_PRIMARY)),
            border: iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .into()
}

// =============================================================================
// Settings "tab" — horizontal panel
// =============================================================================

// Keyboard event handling (global, like original Kortina)
// =============================================================================

fn keyboard_event_to_message(k: &iced::keyboard::Event, status: iced::event::Status) -> Message {
    use iced::keyboard::{Event as KE, Key};

    let (keybinds, capture) = crate::keybinds::live();

    match k {
        KE::ModifiersChanged(mods) => {
            Message::Event(iced::Event::Keyboard(KE::ModifiersChanged(*mods)))
        }
        KE::KeyPressed { key, modifiers, text, .. } => {
            // Shortcut-rebinding capture: swallow the next key press as the
            // new combo (Esc cancels).
            if let Some(action) = capture {
                if let Key::Named(iced::keyboard::key::Named::Escape) = key {
                    return Message::KeyBindCancel;
                }
                let key_part = match key {
                    Key::Character(s) => s.to_lowercase(),
                    Key::Named(named) => crate::keybinds::named_key_name(*named).to_string(),
                    _ => return Message::Nop,
                };
                if key_part.is_empty() {
                    return Message::Nop;
                }
                let mut combo = String::new();
                if modifiers.control() {
                    combo.push_str("ctrl+");
                }
                if modifiers.alt() {
                    combo.push_str("alt+");
                }
                if modifiers.shift() {
                    combo.push_str("shift+");
                }
                combo.push_str(&key_part);
                return Message::KeyBindSet(action, combo);
            }

            // User-configurable shortcuts take priority.
            for (action, combo) in keybinds {
                if crate::keybinds::matches(combo.as_str(), *modifiers, key) {
                    return match action {
                        crate::keybinds::Action::Copy => Message::TermCopy,
                        crate::keybinds::Action::Paste => Message::TermPaste,
                        crate::keybinds::Action::Search => Message::SearchOpen,
                        crate::keybinds::Action::QuickToggle => Message::QuickToggle,
                        crate::keybinds::Action::NewTerm => Message::TermNew,
                        crate::keybinds::Action::CloseTerm => Message::TermCloseActive,
                        crate::keybinds::Action::Clear => Message::TermClear,
                        crate::keybinds::Action::NextTab => Message::TermSelectNext,
                        crate::keybinds::Action::PrevTab => Message::TermSelectPrev,
                        crate::keybinds::Action::Settings => Message::SettingsOpen,
                    };
                }
            }

            // If a widget already handled the key, don't forward to terminal
            if status == iced::event::Status::Captured {
                return Message::Nop;
            }

            // Ctrl+letter → control character
            if modifiers.control() && !modifiers.shift() {
                if let Key::Character(s) = key {
                    if let Some(ch) = s.chars().next() {
                        let lc = ch.to_ascii_lowercase();
                        if lc.is_ascii_lowercase() {
                            return Message::TermWrite(vec![(lc as u8) - (b'a' - 1)]);
                        }
                    }
                }
            }

            // Special keys → escape sequences
            if let Key::Named(named) = key {
                if let Some(seq) = named_key_to_seq(*named) {
                    return Message::TermWrite(seq.into_bytes());
                }
            }

            // Regular printable key
            if let Some(t) = text {
                if !t.is_empty() {
                    return Message::TermWrite(t.as_bytes().to_vec());
                }
            }

            Message::Nop
        }
        KE::KeyReleased { .. } => Message::Nop,
    }
}

fn named_key_to_seq(named: iced::keyboard::key::Named) -> Option<String> {
    use iced::keyboard::key::Named;

    let seq = match named {
        Named::Enter => "\r".to_string(),
        Named::Backspace => "\x7f".to_string(),
        Named::Tab => "\t".to_string(),
        Named::Escape => "\x1b".to_string(),
        Named::ArrowUp => "\x1b[A".to_string(),
        Named::ArrowDown => "\x1b[B".to_string(),
        Named::ArrowRight => "\x1b[C".to_string(),
        Named::ArrowLeft => "\x1b[D".to_string(),
        Named::Home => "\x1b[H".to_string(),
        Named::End => "\x1b[F".to_string(),
        Named::Delete => "\x1b[3~".to_string(),
        Named::PageUp => "\x1b[5~".to_string(),
        Named::PageDown => "\x1b[6~".to_string(),
        Named::Insert => "\x1b[2~".to_string(),
        _ => return None,
    };
    Some(seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_chars_and_ascii() {
        assert!(is_wide_char('中'));
        assert!(is_wide_char('あ'));
        assert!(is_wide_char('한'));
        assert!(!is_wide_char('A'));
        assert!(!is_wide_char('→')); // ambiguous width, treated narrow
        assert!(!is_wide_char(' '));
    }

    #[test]
    fn title_cell_width_counts_wide_as_two() {
        assert_eq!(title_cell_width(""), 0);
        assert_eq!(title_cell_width("ab"), 2);
        assert_eq!(title_cell_width("中文"), 4);
        assert_eq!(title_cell_width("中文ab"), 6);
    }

    #[test]
    fn expand_path_handles_home_absolute_and_empty() {
        assert_eq!(expand_path(""), None);
        assert_eq!(
            expand_path("/tmp"),
            Some(std::path::PathBuf::from("/tmp"))
        );
        if dirs::home_dir().is_some() {
            assert!(expand_path("~/").is_some());
        }
    }

    #[test]
    fn percent_decode_decodes_escapes() {
        assert_eq!(percent_decode("/home/a%20b"), "/home/a b");
        assert_eq!(percent_decode("%E4%B8%AD.txt"), "中.txt");
        // Invalid escapes pass through untouched.
        assert_eq!(percent_decode("100%ZZ"), "100%ZZ");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn classify_urls_and_file_urls() {
        assert_eq!(
            classify_jump_target("https://example.com"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            classify_jump_target("(https://example.com),"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            classify_jump_target("file:///usr/share/doc"),
            Some("/usr/share/doc".to_string())
        );
        // Percent-encoded paths decode before hitting the filesystem.
        let dir = std::env::temp_dir().join(format!("korterm-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.txt");
        std::fs::write(&file, "x").unwrap();
        let word = format!("file://{}/a%20b.txt", dir.display());
        assert_eq!(
            classify_jump_target(&word),
            Some(file.to_string_lossy().into_owned())
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(classify_jump_target(""), None);
    }

    #[test]
    fn classify_file_with_line_number() {
        // path:line classification probes the filesystem, so use a real
        // temp file.
        let dir = std::env::temp_dir().join(format!("korterm-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let word = format!("{}/main.rs:42", dir.display());
        assert_eq!(
            classify_jump_target(&word),
            Some(file.to_string_lossy().into_owned())
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn classify_rejects_nonexistent_paths() {
        assert_eq!(classify_jump_target("/no/such/path/ornithopter:12"), None);
        assert_eq!(classify_jump_target(":::"), None);
    }
}