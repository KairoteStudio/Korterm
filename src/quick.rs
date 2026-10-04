// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Standalone Quake-style quick terminal: `korterm --quick`.
//!
//! Opens a borderless, always-on-top window covering the top half of the
//! monitor. Running `korterm --quick` again while it is open
//! closes it (toggle) via a Unix-socket IPC handshake — bind that command
//! to a system shortcut (e.g. Ctrl+`) in your desktop environment.
//!
//! Wayland notes: compositors don't let clients set global hotkeys, so the
//! toggle key must be bound in system settings. This process uses the X11
//! backend (XWayland) so the window can be positioned at the top-left of
//! the screen and kept always-on-top; pure-Wayland windows can do neither.

use iced::widget::{container, mouse_area, text};
use iced::window;
use iced::{Element, Length, Size, Task, Theme};

use crate::theme;

/// Handoff slot for the pre-bound single-instance socket: `run()` binds
/// it before iced starts (atomic lock), the IPC task picks it up here.
static QUICK_LISTENER: std::sync::Mutex<Option<std::os::unix::net::UnixListener>> =
    std::sync::Mutex::new(None);

/// Path of the single-instance toggle socket. `$XDG_RUNTIME_DIR` is a
/// user-owned tmpfs; falling back to the user's cache dir keeps the
/// socket out of world-writable `/tmp`, where any local user could
/// pre-create it and permanently break the toggle for the real user.
fn quick_socket_path() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return Some(std::path::Path::new(&dir).join("korterm-quick.sock"));
        }
    }
    let dir = dirs::cache_dir()?.join("korterm");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("korterm-quick.sock"))
}

/// Handoff slot for the asynchronously spawned terminal: widgets require
/// `Message: Clone` and a PTY must never be duplicated, so the Terminal
/// travels through here instead of inside the message.
static PENDING_QUICK_TERM: std::sync::Mutex<Option<Box<terminal::Terminal>>> =
    std::sync::Mutex::new(None);

const QUICK_WM_NAME: &str = "Korterm Quick Terminal";

/// Opacity fade duration (ms).
const FADE_MS: f32 = 160.0;

/// Set `_NET_WM_WINDOW_OPACITY` on an X11 window — KWin honors this EWMH
/// property, which lets us hide the window WITHOUT unmapping it (no KWin
/// open/close animations, always-on-top is never lost). `value`: 0..=255.
fn set_x11_opacity(xwin: u32, value: u8) {
    let _ = std::process::Command::new("xprop")
        .arg("-id")
        .arg(xwin.to_string())
        .arg("-f")
        .arg("_NET_WM_WINDOW_OPACITY")
        .arg("32c")
        .arg("-set")
        .arg("_NET_WM_WINDOW_OPACITY")
        .arg(format!("0x{:08x}", value as u32 * 0x0101_0101))
        .status();
}

/// Find this process' X11 window id by its WM_NAME (via `xprop`).
fn find_xwin() -> Option<u32> {
    let out = std::process::Command::new("xprop")
        .arg("-root")
        .arg("_NET_CLIENT_LIST")
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    for tok in s.split([',', ' ', '\n', '\t']) {
        let Some(hex) = tok.trim().strip_prefix("0x") else {
            continue;
        };
        let Ok(id) = u32::from_str_radix(hex, 16) else {
            continue;
        };
        if let Ok(n) = std::process::Command::new("xprop")
            .arg("-id")
            .arg(id.to_string())
            .arg("WM_NAME")
            .output()
        {
            if String::from_utf8_lossy(&n.stdout).contains(QUICK_WM_NAME) {
                return Some(id);
            }
        }
    }
    None
}

pub struct QuickState {
    window: Option<window::Id>,
    term: Option<terminal::Terminal>,
    content_size: (f32, f32),
    selecting: bool,
    scale: f32,
    monitor: Option<Size>,
    focused: bool,
    modifiers: iced::keyboard::Modifiers,
    /// Whether the window is currently hidden (opacity 0, parked 1x1).
    hidden: bool,
    /// X11 window id, found lazily via xprop (for opacity control).
    xwin: Option<u32>,
    /// Opacity fade animation: (elapsed ms, from, to), values in 0..1.
    fade: Option<(f32, f32, f32)>,
    /// True when the last hide fell back to Mode::Hidden (xprop missing).
    fallback_hidden: bool,
    /// Right-click context menu open?
    menu_open: bool,
    /// Menu reveal progress 0..1 (animated open/close).
    menu_t: f32,
    /// Menu anchor (click position, clamped).
    menu_pos: (f32, f32),
    /// Last known mouse position over the panel.
    mouse_pos: (f32, f32),
    /// A PTY flush is already scheduled (resize debounce in flight).
    resize_pending: bool,
}

pub enum Message {
    /// A raw key press plus whether a widget had already consumed it.
    /// Resolved in `update()` because the escape sequence depends on the
    /// terminal's DECCKM mode, which the event listener cannot see.
    KeyPress(iced::keyboard::Event, iced::event::Status),
    Ready(window::Id),
    Monitor(Option<Size>),
    Scale(f32),
    Spawned,
    Resize(usize, usize, f32, f32),
    FlushResize,
    SelectPress,
    SelectMove(f32, f32),
    SelectRelease,
    Write(Vec<u8>),
    /// File dropped from the file manager → insert its quoted path.
    FileDropped(std::path::PathBuf),
    Copy,
    Paste,
    SelectAll,
    Pump,
    Tick,
    MenuToggle,
    FocusGained,
    FocusLost,
    IpcToggle,
    Modifiers(iced::keyboard::Modifiers),
    Exit,
    /// No-op — e.g. clipboard read returned nothing usable.
    Nop,
}

impl Clone for Message {
    fn clone(&self) -> Self {
        match self {
            Message::KeyPress(k, st) => Message::KeyPress(k.clone(), *st),
            Message::Ready(id) => Message::Ready(*id),
            Message::Monitor(s) => Message::Monitor(*s),
            Message::Scale(s) => Message::Scale(*s),
            Message::Spawned => Message::Spawned,
            Message::Resize(c, r, w, h) => Message::Resize(*c, *r, *w, *h),
            Message::FlushResize => Message::FlushResize,
            Message::SelectPress => Message::SelectPress,
            Message::SelectMove(x, y) => Message::SelectMove(*x, *y),
            Message::SelectRelease => Message::SelectRelease,
            Message::Write(b) => Message::Write(b.clone()),
            Message::FileDropped(p) => Message::FileDropped(p.clone()),
            Message::Copy => Message::Copy,
            Message::Paste => Message::Paste,
            Message::SelectAll => Message::SelectAll,
            Message::Pump => Message::Pump,
            Message::Tick => Message::Tick,
            Message::MenuToggle => Message::MenuToggle,
            Message::FocusGained => Message::FocusGained,
            Message::FocusLost => Message::FocusLost,
            Message::IpcToggle => Message::IpcToggle,
            Message::Modifiers(m) => Message::Modifiers(*m),
            Message::Exit => Message::Exit,
            Message::Nop => Message::Nop,
        }
    }
}

pub struct QuickProgram;

impl iced::Program for QuickProgram {
    type State = QuickState;
    type Message = Message;
    type Theme = Theme;
    type Renderer = iced::Renderer;
    type Executor = iced::executor::Default;

    fn name() -> &'static str {
        "Korterm Quick Terminal"
    }

    fn settings(&self) -> iced::Settings {
        iced::Settings {
            default_font: iced::Font::DEFAULT,
            antialiasing: true,
            ..Default::default()
        }
    }

    fn window(&self) -> Option<window::Settings> {
        Some(window::Settings {
            size: Size::new(800.0, 450.0),
            resizable: true,
            decorations: false,
            exit_on_close_request: true,
            // Stays above other windows (honored on X11/XWayland; some
            // compositors also honor it for XWayland windows).
            level: window::Level::AlwaysOnTop,
            ..Default::default()
        })
    }

    fn boot(&self) -> (Self::State, iced::Task<Self::Message>) {
        let spawn = Task::perform(
            terminal::Terminal::with_shell_async(80, 24, None, None),
            |term| {
                // Park the terminal in the handoff slot; the (clone-safe)
                // Spawned message tells `update` to pick it up.
                *PENDING_QUICK_TERM
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(term));
                Message::Spawned
            },
        );
        let ready = window::latest().map(|opt| {
            opt.map(Message::Ready).unwrap_or(Message::Exit)
        });
        (
            QuickState {
                window: None,
                term: None,
                content_size: (0.0, 0.0),
                selecting: false,
                scale: 1.0,
                monitor: None,
                focused: false,
                modifiers: iced::keyboard::Modifiers::empty(),
                hidden: false,
                xwin: None,
                fade: None,
                fallback_hidden: false,
                menu_open: false,
                menu_t: 0.0,
                menu_pos: (8.0, 8.0),
                mouse_pos: (8.0, 8.0),
                resize_pending: false,
            },
            iced::Task::batch([ready, spawn]),
        )
    }

    fn update(
        &self,
        state: &mut Self::State,
        message: Self::Message,
    ) -> iced::Task<Self::Message> {
        match message {
            // The quick terminal ignores capture status on purpose: it has
            // no focusable widgets that would legitimately eat a key.
            Message::KeyPress(k, _status) => {
                let app_cursor_keys = state
                    .term
                    .as_ref()
                    .is_some_and(|t| t.buf.application_cursor_keys);
                if let Some(msg) = key_to_message(&k, app_cursor_keys) {
                    return self.update(state, msg);
                }
            }
            Message::Ready(id) => {
                state.window = Some(id);
                // Locate our X11 window once; then open with a fade-in
                // instead of KWin's map animation.
                state.xwin = find_xwin();
                if let Some(xw) = state.xwin {
                    set_x11_opacity(xw, 0);
                    state.fade = Some((0.0, 0.0, 1.0));
                }
                return Task::batch([
                    window::scale_factor(id).map(Message::Scale),
                    window::monitor_size(id).map(Message::Monitor),
                ]);
            }
            Message::Scale(s) => {
                state.scale = s;
                return apply_configure(state);
            }
            Message::Monitor(size) => {
                state.monitor = size;
                return apply_configure(state);
            }
            Message::Spawned => {
                if let Some(term) = PENDING_QUICK_TERM
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                {
                    state.term = Some(*term);
                }
            }
            Message::Resize(cols, rows, w, h) => {
                // While hidden the window is 1x1 — don't let that shrink the
                // PTY or lose the real content size (TUI apps keep running).
                if state.hidden {
                    return Task::none();
                }
                if state.content_size != (w, h) {
                    state.content_size = (w, h);
                    if let Some(term) = &mut state.term {
                        // The buffer resizes immediately; the PTY flush is
                        // debounced — flushing one SIGWINCH per pixel while
                        // an edge-drag floods Resize events garbles TUI
                        // redraws (the main window applies the same 150ms
                        // debounce for exactly this reason).
                        term.resize(cols, rows);
                        if !state.resize_pending {
                            state.resize_pending = true;
                            return Task::perform(
                                async {
                                    tokio::time::sleep(std::time::Duration::from_millis(150))
                                        .await;
                                },
                                |_| Message::FlushResize,
                            );
                        }
                    }
                }
            }
            Message::FlushResize => {
                state.resize_pending = false;
                if let Some(term) = &mut state.term {
                    term.flush_pty_resize();
                }
            }
            Message::SelectPress => {
                state.selecting = true;
                state.menu_open = false;
                if let Some(term) = &mut state.term {
                    term.clear_selection();
                }
            }
            Message::SelectMove(x, y) => {
                state.mouse_pos = (x, y);
                if state.selecting {
                    if let Some(term) = &mut state.term {
                        let (cx, vy) = terminal::widget::pixel_to_cell(term, x, y);
                        if term.selection.is_none() {
                            term.start_selection(cx, vy);
                        } else {
                            term.extend_selection(cx, vy);
                        }
                    }
                }
            }
            Message::SelectRelease => {
                state.selecting = false;
            }
            Message::Write(bytes) => {
                if let Some(term) = &mut state.term {
                    term.input(&bytes);
                }
            }
            Message::Copy => {
                state.menu_open = false;
                if let Some(term) = &state.term {
                    let selected = term.selected_text();
                    if !selected.is_empty() {
                        return iced::clipboard::write(selected);
                    }
                }
            }
            Message::Paste => {
                state.menu_open = false;
                return iced::clipboard::read().map(|s| {
                    s.map(|t| Message::Write(t.into_bytes()))
                        // Clipboard holds no text (image/file/failed read):
                        // do nothing. The old code exited the whole quick
                        // terminal here, killing the shell session with it.
                        .unwrap_or(Message::Nop)
                });
            }
            Message::SelectAll => {
                state.menu_open = false;
                if let Some(term) = &mut state.term {
                    term.select_all();
                }
            }
            Message::Pump => {
                let mut alive = true;
                if let Some(term) = &mut state.term {
                    term.pump();
                    // Drive the smooth cursor animation + blink — without
                    // this the cursor freezes at stale positions (the main
                    // app does the same in its AnimTick).
                    term.advance_cursor_anim(33.0);
                    term.tick_blink(true);
                    // Shell exited (e.g. the user typed `exit`): close the
                    // overlay instead of leaving a dead PTY that swallows
                    // every keystroke silently.
                    alive = term.is_alive();
                }
                if !alive {
                    return self.update(state, Message::Exit);
                }
            }
            Message::Nop => {}
            Message::Tick => {
                // Opacity fade animation (show/hide). The window is NEVER
                // unmapped or moved off-screen, so KWin never plays its
                // open/close animations and always-on-top is never lost.
                if let Some((t, from, to)) = &mut state.fade {
                    *t += 16.0;
                    let p = (*t / FADE_MS).clamp(0.0, 1.0);
                    let e = 1.0 - (1.0 - p) * (1.0 - p) * (1.0 - p);
                    let v = *from + (*to - *from) * e;
                    if let Some(xw) = state.xwin {
                        set_x11_opacity(xw, (v * 255.0) as u8);
                    }
                    if p >= 1.0 {
                        let faded_out = *to == 0.0;
                        state.fade = None;
                        // Fade-out finished: shrink to 1x1 and park in the
                        // bottom-left corner so the invisible window can't
                        // intercept clicks or cover anything.
                        if faded_out {
                            if let Some(id) = state.window {
                                let mon_h = state
                                    .monitor
                                    .map(|m| m.height)
                                    .unwrap_or(800.0)
                                    / state.scale.max(0.5);
                                return Task::batch([
                                    window::move_to(
                                        id,
                                        iced::Point::new(0.0, (mon_h - 2.0).max(0.0)),
                                    ),
                                    window::resize(id, Size::new(1.0, 1.0)),
                                ]);
                            }
                        }
                    }
                }
                // Menu open/close animation progress.
                let target = if state.menu_open { 1.0 } else { 0.0 };
                let speed = 16.0 / 120.0;
                if state.menu_t < target {
                    state.menu_t = (state.menu_t + speed).min(1.0);
                } else if state.menu_t > target {
                    state.menu_t = (state.menu_t - speed).max(0.0);
                }
            }
            Message::Modifiers(m) => {
                state.modifiers = m;
            }
            Message::FocusGained => {
                eprintln!("[quick] FocusGained");
                state.focused = true;
                // Hidden (1x1, invisible) but got focused — e.g. the user
                // clicked its single pixel or the taskbar entry. Pop back.
                if state.hidden {
                    return show_quick(state);
                }
            }
            Message::FocusLost => {
                // Only retract on a real focused→unfocused transition:
                // clicking any other window fades the overlay out.
                eprintln!("[quick] FocusLost (focused={}, hidden={})", state.focused, state.hidden);
                let was = state.focused;
                state.focused = false;
                if was && !state.hidden {
                    return hide_quick(state);
                }
            }
            Message::FileDropped(path) => {
                // Same behaviour as the main window: drop a file in and
                // its shell-quoted path lands on the prompt.
                let alive = state
                    .term
                    .as_mut()
                    .map(|t| t.is_alive())
                    .unwrap_or(false);
                if alive {
                    let mut text = terminal::shell_quote_path(&path);
                    text.push(' ');
                    if let Some(term) = state.term.as_mut() {
                        term.input(text.as_bytes());
                    }
                }
            }
            Message::IpcToggle => {
                eprintln!("[quick] IpcToggle (hidden={} -> {})", state.hidden, !state.hidden);
                state.hidden = !state.hidden;
                if state.hidden {
                    return hide_quick(state);
                } else if state.window.is_some() {
                    return show_quick(state);
                }
            }
            Message::MenuToggle => {
                state.menu_open = !state.menu_open;
                if state.menu_open {
                    let (mw, mh) = state.content_size;
                    let (mx, my) = state.mouse_pos;
                    // Clamp so the menu stays inside the panel.
                    state.menu_pos = (
                        mx.min((mw - 150.0).max(0.0)),
                        my.min((mh - 100.0).max(0.0)),
                    );
                }
            }
            Message::Exit => {
                if let Some(id) = state.window {
                    return window::close(id);
                }
            }
        }
        Task::none()
    }

    fn view<'a>(
        &self,
        state: &'a Self::State,
        _window: window::Id,
    ) -> Element<'a, Self::Message, Self::Theme, Self::Renderer> {
        let body: Element<'_, Message> = match &state.term {
            Some(term) => mouse_area(terminal::widget::terminal_view(
                term,
                true,
                iced::widget::Id::new("quick-terminal-view"),
                Message::Resize,
            ))
            .on_press(Message::SelectPress)
            .on_move(|p| Message::SelectMove(p.x, p.y))
            .on_release(Message::SelectRelease)
            .on_right_press(Message::MenuToggle)
            .interaction(iced::mouse::Interaction::Text)
            .into(),
            None => container(
                text("正在启动快速终端…").size(13.0).color(theme::DIM),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into(),
        };

        // Right-click context menu — anchored at the click point and
        // animated (grows from the anchor, shrinks on close).
        let body: Element<'_, Message> = if state.menu_t > 0.02 {
            let e = crate::animation::ease_out_cubic(state.menu_t);
            // Natural size of the panel (3 rows + gaps + padding): the box
            // below is sized from it so the card never paints an empty strip.
            let menu_w = QUICK_MENU_W * e;
            let menu_h = QUICK_MENU_H * e;
            let (mx, my) = state.menu_pos;
            let menu = container(
                iced::widget::column![
                    menu_entry("复制", Message::Copy),
                    menu_entry("粘贴", Message::Paste),
                    menu_entry("全选", Message::SelectAll),
                ]
                .spacing(MENU_SPACING)
                .padding([MENU_PAD_Y, 4.0]),
            )
            .width(Length::Fixed(QUICK_MENU_W))
            .height(Length::Fixed(QUICK_MENU_H))
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::BG_PRIMARY)),
                border: iced::Border {
                    color: theme::BORDER,
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            })
            .clip(true);
            let anchored = container(menu)
                .width(Length::Fixed(menu_w.max(1.0)))
                .height(Length::Fixed(menu_h.max(1.0)))
                .clip(true);
            iced::widget::stack![
                body,
                container(anchored)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(iced::Padding {
                        top: my,
                        right: 0.0,
                        bottom: 0.0,
                        left: mx,
                    })
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Top),
            ]
            .into()
        } else {
            body
        };

        // Keep the input method alive over the terminal (Chinese input) —
        // but disable it while Ctrl is held, or the IME layer swallows
        // Ctrl+` before the app ever sees the key.
        let body = crate::animation::ImeZone::new(body).ime(
            !state.modifiers.control(),
            String::new(),
            None,
        );

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::BG_PANEL)),
                border: iced::Border {
                    color: theme::BORDER,
                    width: 1.0,
                    radius: iced::border::Radius {
                        top_left: 0.0_f32,
                        top_right: 0.0_f32,
                        bottom_left: 10.0_f32,
                        bottom_right: 10.0_f32,
                    },
                },
                ..Default::default()
            })
            .clip(true)
            .into()
    }

    fn title(&self, _state: &Self::State, _window: window::Id) -> String {
        String::from("Korterm Quick Terminal")
    }

    fn subscription(&self, _state: &Self::State) -> iced::Subscription<Self::Message> {
        // ~30 fps PTY pump — without it, shell output (echo, prompts) is
        // never read back into the buffer and typing appears dead.
        let pump = iced::time::every(std::time::Duration::from_millis(33))
            .map(|_| Message::Pump);
        // 60 fps tick for the menu open/close animation.
        let tick = iced::time::every(std::time::Duration::from_millis(16))
            .map(|_| Message::Tick);

        iced::Subscription::batch([
            // Keyboard regardless of capture status + focus tracking
            // (focus lost → the overlay retracts).
            iced::event::listen_with(|event, status, _id| match &event {
                iced::Event::Keyboard(k) => Some(Message::KeyPress(k.clone(), status)),
                iced::Event::InputMethod(iced::advanced::input_method::Event::Commit(content)) => {
                    Some(Message::Write(content.clone().into_bytes()))
                }
                iced::Event::Window(window::Event::Focused) => {
                    Some(Message::FocusGained)
                }
                iced::Event::Window(window::Event::Unfocused) => {
                    Some(Message::FocusLost)
                }
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    Some(Message::FileDropped(path.clone()))
                }
                _ => None,
            }),
            // IPC toggle listener: another `--quick` invocation connects
            // to the socket to show/hide this window.
            iced::Subscription::run(ipc_stream),
            pump,
            tick,
        ])
    }

    fn theme(
        &self,
        _state: &Self::State,
        _window: window::Id,
    ) -> Option<Self::Theme> {
        Some(Theme::Dark)
    }
}

/// Position + size the window once monitor size and scale are known:
/// top-left anchored, half the monitor height. `monitor_size` reports
/// PHYSICAL pixels while `resize` takes logical ones — hence the division.
fn apply_configure(state: &mut QuickState) -> Task<Message> {
    if let (Some(id), Some(size)) = (state.window, state.monitor) {
        let s = state.scale.max(0.5);
        return Task::batch([
            window::resize(id, Size::new(size.width / s, size.height / 2.0 / s)),
            window::move_to(id, iced::Point::new(0.0, 0.0)),
        ]);
    }
    Task::none()
}

/// Hide the quick terminal: fade opacity to 0 (window stays mapped — no
/// KWin animations, always-on-top persists), then Tick shrinks it to 1x1
/// in the corner. Falls back to Mode::Hidden if xprop is unavailable.
fn hide_quick(state: &mut QuickState) -> Task<Message> {
    if state.xwin.is_none() {
        state.xwin = find_xwin();
    }
    if state.xwin.is_some() {
        state.fade = Some((0.0, 1.0, 0.0));
        Task::none()
    } else if let Some(id) = state.window {
        eprintln!("[quick] xprop unavailable — falling back to Mode::Hidden");
        state.fallback_hidden = true;
        window::set_mode(id, window::Mode::Hidden)
    } else {
        Task::none()
    }
}

/// Show the quick terminal: restore geometry while still invisible, then
/// fade in and grab focus.
fn show_quick(state: &mut QuickState) -> Task<Message> {
    let Some(id) = state.window else {
        return Task::none();
    };
    if state.fallback_hidden {
        state.fallback_hidden = false;
        return Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::gain_focus(id),
        ]);
    }
    let w = state.content_size.0.max(600.0);
    let h = state.content_size.1.max(400.0);
    if let Some(xw) = state.xwin {
        set_x11_opacity(xw, 0);
    }
    state.fade = Some((0.0, 0.0, 1.0));
    Task::batch([
        window::resize(id, Size::new(w, h)),
        window::move_to(id, iced::Point::new(0.0, 0.0)),
        window::gain_focus(id),
    ])
}

/// The IPC toggle stream: another `--quick` invocation connects to the
/// socket to show/hide this window.
fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(100, ipc_task)
}

async fn ipc_task(mut out: iced::futures::channel::mpsc::Sender<Message>) {
    // The listener was bound in `run()` — before iced started — as the
    // atomic single-instance lock; pick it up here.
    let listener = QUICK_LISTENER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let Some(listener) = listener else {
        return;
    };
    let _ = listener.set_nonblocking(true);
    loop {
        match listener.accept() {
            Ok(_) => {
                eprintln!("[quick] ipc: toggle connection accepted");
                let _ = out.try_send(Message::IpcToggle);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    }
}

/// Context menu metrics — the animated box clips the panel, so it must match
/// the panel's natural size. iced lays text out at 1.3× the font size by
/// default (`LineHeight`); a row is its 12px label plus `[6, 10]` padding.
const QUICK_MENU_ROWS: usize = 3;
const QUICK_MENU_W: f32 = 140.0;
const MENU_SPACING: f32 = 2.0;
const MENU_PAD_Y: f32 = 6.0;
const QUICK_MENU_H: f32 = QUICK_MENU_ROWS as f32 * (12.0 * 1.3 + 6.0 * 2.0)
    + MENU_SPACING * (QUICK_MENU_ROWS - 1) as f32
    + MENU_PAD_Y * 2.0;

fn menu_entry(label: &str, msg: Message) -> iced::Element<'static, Message> {
    let label = label.to_string();
    iced::widget::button(text(label).size(12.0).color(theme::TEXT))
        .width(Length::Fill)
        .padding([6.0, 10.0])
        .on_press(msg)
        .style(move |_t, st| match st {
            iced::widget::button::Status::Hovered
            | iced::widget::button::Status::Pressed => iced::widget::button::Style {
                background: Some(iced::Background::Color(theme::BG_ELEVATED)),
                ..Default::default()
            },
            _ => iced::widget::button::Style::default(),
        })
        .into()
}

/// Map a keyboard event to a quick-terminal message. Ctrl+` closes the
/// window (matches the system shortcut used to open it).
fn key_to_message(k: &iced::keyboard::Event, app_cursor_keys: bool) -> Option<Message> {
    use iced::keyboard::{Event as KE, Key};

    match k {
        KE::ModifiersChanged(m) => Some(Message::Modifiers(*m)),
        KE::KeyPressed { key, modifiers, text, .. } => {
            if modifiers.control() {
                if let Key::Character(s) = key {
                    if s == "`" {
                        // Hide (minimize) — the session stays alive.
                        return Some(Message::IpcToggle);
                    }
                    if let Some(ch) = s.chars().next() {
                        let lc = ch.to_ascii_lowercase();
                        if modifiers.shift() {
                            if lc == 'c' {
                                return Some(Message::Copy);
                            }
                            if lc == 'v' {
                                return Some(Message::Paste);
                            }
                        }
                        if lc.is_ascii_lowercase() {
                            return Some(Message::Write(vec![(lc as u8) - (b'a' - 1)]));
                        }
                    }
                }
            }
            if let Key::Named(named) = key {
                if let Some(seq) = named_seq(*named, app_cursor_keys) {
                    return Some(Message::Write(seq.into_bytes()));
                }
            }
            if let Some(t) = text {
                if !t.is_empty() {
                    return Some(Message::Write(t.as_bytes().to_vec()));
                }
            }
            None
        }
        _ => None,
    }
}

fn named_seq(named: iced::keyboard::key::Named, app_cursor_keys: bool) -> Option<String> {
    use iced::keyboard::key::Named;

    // DECCKM (?1): cursor keys and Home/End switch to their SS3 form
    // (`ESC O x`). Programs that enabled it ignore the plain CSI form,
    // which is what makes Home/End look dead in full-screen programs.
    if app_cursor_keys {
        let ss3 = match named {
            Named::ArrowUp => "A",
            Named::ArrowDown => "B",
            Named::ArrowRight => "C",
            Named::ArrowLeft => "D",
            Named::Home => "H",
            Named::End => "F",
            _ => "",
        };
        if !ss3.is_empty() {
            return Some(format!("\x1bO{ss3}"));
        }
    }

    let seq = match named {
        Named::Enter => "\r",
        Named::Backspace => "\x7f",
        Named::Tab => "\t",
        Named::Escape => "\x1b",
        Named::ArrowUp => "\x1b[A",
        Named::ArrowDown => "\x1b[B",
        Named::ArrowRight => "\x1b[C",
        Named::ArrowLeft => "\x1b[D",
        Named::Home => "\x1b[H",
        Named::End => "\x1b[F",
        Named::Delete => "\x1b[3~",
        Named::PageUp => "\x1b[5~",
        Named::PageDown => "\x1b[6~",
        Named::Insert => "\x1b[2~",
        _ => return None,
    };
    Some(seq.to_string())
}

/// Entry point for `--quick`.
///
/// Toggle protocol: a Unix socket in `$XDG_RUNTIME_DIR` acts as the
/// single-instance lock. If another quick-terminal instance is already
/// listening, this process just signals it to exit (toggle-off) and quits.
/// Bind `korterm --quick` to a system shortcut for Ctrl+`.
pub fn run() -> iced::Result {
    use std::os::unix::net::UnixStream;

    // Native Wayland cannot position windows or keep them on top, so the
    // quick terminal runs through XWayland (X11) instead: drop the
    // Wayland connection env so winit falls back to X11 (winit 0.30
    // ignores WINIT_UNIX_BACKEND and prefers Wayland whenever its socket
    // is present). The main terminal app is unaffected.
    std::env::remove_var("WAYLAND_DISPLAY");
    std::env::remove_var("WAYLAND_SOCKET");

    // X11 IME (XIM) needs XMODIFIERS pointing at the fcitx5 XIM server;
    // the Wayland session doesn't set it (fcitx5 there uses the Wayland
    // protocol instead). If Chinese input still fails, make sure the
    // "X Input Method" frontend is enabled in fcitx5's addon settings.
    if std::env::var("XMODIFIERS")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        std::env::set_var("XMODIFIERS", "@im=fcitx");
    }

    let sock_path = quick_socket_path().unwrap_or_else(|| {
        eprintln!("[quick-child] no usable socket directory — toggle IPC disabled");
        std::process::exit(0);
    });

    // Single-instance toggle: bind the socket BEFORE iced starts so the
    // lock is atomic. (The old flow probed with connect() and only bound
    // inside the IPC task seconds later — two instances launched in that
    // window would both start and clobber each other's socket.)
    let listener = match std::os::unix::net::UnixListener::bind(&sock_path) {
        Ok(l) => Some(l),
        Err(_) => {
            // Bind failed: a live instance holds it, or a stale file from
            // a crashed one. Toggle the live instance first.
            if UnixStream::connect(&sock_path).is_ok() {
                eprintln!("[quick-child] connected to running instance — toggle sent");
                std::process::exit(0);
            }
            // Nobody listening → stale socket. Clean it and retry once;
            // losing THAT bind means another primary won the race.
            let _ = std::fs::remove_file(&sock_path);
            match std::os::unix::net::UnixListener::bind(&sock_path) {
                Ok(l) => Some(l),
                Err(_) => {
                    if UnixStream::connect(&sock_path).is_ok() {
                        eprintln!("[quick-child] lost bind race — toggle sent");
                    }
                    std::process::exit(0);
                }
            }
        }
    };
    eprintln!("[quick-child] no running instance — starting new window");
    *QUICK_LISTENER
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = listener;

    // First run: open the system shortcut settings so the user can bind
    // the global hotkey (Wayland/X11 apps cannot register global hotkeys
    // themselves; the compositor owns them).
    crate::config::migrate_legacy();
    let marker = dirs::config_dir()
        .map(|d| d.join("korterm/quick-hotkey-hint-shown"));
    let needs_hint = marker
        .as_ref()
        .map(|p| !p.exists())
        .unwrap_or(false);
    if needs_hint {
        // Install a .desktop entry so the quick terminal shows up in the
        // desktop environment's shortcut/app pickers.
        let exe = std::env::current_exe().ok();
        if let (Some(exe), Some(apps_dir)) = (exe, dirs::data_dir()) {
            let desktop = apps_dir.join("applications/korterm-quick.desktop");
            let content = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=Korterm Quick Terminal\n\
                 Comment=Toggle the Korterm quick terminal overlay\n\
                 Exec=\"{}\" --quick\n\
                 Icon=utilities-terminal\n\
                 Categories=System;TerminalEmulator;\n",
                exe.display().to_string().replace('%', "%%")
            );
            if let Some(dir) = desktop.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&desktop, content);
        }

        // Open the system shortcut settings page directly.
        let de = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let settings: Option<(&str, Vec<&str>)> = if de.contains("KDE") {
            Some(("systemsettings", vec!["kcm_keys"]))
        } else if de.contains("GNOME") {
            Some(("gnome-control-center", vec!["keyboard"]))
        } else {
            None
        };
        if let Some((prog, args)) = settings {
            let _ = std::process::Command::new(prog)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        if let Some(path) = &marker {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, b"1");
        }
    }

    iced_winit::run(QuickProgram).map_err(iced::Error::from)
}
