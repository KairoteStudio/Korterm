// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Settings page — desktop-standard "left nav + right content" layout
//! (à la iTerm2 / Warp / VS Code).
//!
//! - Left nav: fixed 210px column of sections with icons.
//! - Right: the active section's form, rows with the label left-aligned
//!   and the control right-aligned, 44px rows.
//! - Glow color: a single color-swatch trigger that opens a popover with
//!   an SV field, a hue slider, preset swatches and a validated hex input.
//! - Brightness: continuous slider + numeric input + wheel fine-tuning.
//! - Shortcuts: kbd-style caps, click to record, Esc cancels.

use iced::widget::{button, container, mouse_area, text, text_input};
use iced::{alignment, Color, Length, Pixels};

use crate::icons::{icon, Icon};
use crate::terminal_panel::{Message, TerminalPanel};
use crate::theme;

pub const GLOW_PRESETS: [u32; 8] = [
    0x4a8cff, 0x00b4d8, 0x2da44e, 0xeac54f, 0xe8590c, 0xe5484d, 0x8250df, 0xd6409f,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Appearance,
    Shell,
    Glow,
    Keybinds,
    System,
    About,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "外观",
            Section::Shell => "Shell",
            Section::Glow => "光晕",
            Section::Keybinds => "快捷键",
            Section::System => "系统",
            Section::About => "关于",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::Appearance => Icon::PanelRight,
            Section::Shell => Icon::Terminal,
            Section::Glow => Icon::LayoutGrid,
            Section::Keybinds => Icon::Square,
            Section::System => Icon::Settings,
            Section::About => Icon::Info,
        }
    }

    pub const ALL: [Section; 6] = [
        Section::Appearance,
        Section::Shell,
        Section::Glow,
        Section::Keybinds,
        Section::System,
        Section::About,
    ];
}

// =============================================================================
// Layout
// =============================================================================

pub fn settings_panel(app: &TerminalPanel) -> iced::Element<'static, Message> {
    // Left nav (fixed) + right content (fills), with a picker overlay.
    let nav = container(
        iced::widget::column(
            Section::ALL
                .iter()
                .map(|s| nav_item(app.settings_section, *s))
                .collect::<Vec<iced::Element<'static, Message>>>(),
        )
        .spacing(2.0)
        .padding([8.0, 6.0]),
    )
    .width(Pixels(210.0))
    .height(Length::Fill)
    .style(|_t| iced::widget::container::Style {
        // Same backdrop as the titlebar: transparent, the window glow
        // shows through; only a hairline border separates the panels.
        background: Some(iced::Background::Color(Color::from_rgba8(0, 0, 0, 0.25))),
        border: iced::Border {
            color: theme::BORDER,
            width: 1.0,
            radius: iced::border::Radius::from(10.0),
        },
        ..Default::default()
    });

    let content = container(match app.settings_section {
        Section::Appearance => appearance_section(app),
        Section::Shell => shell_section(app),
        Section::System => system_section(app),
        Section::Glow => glow_section(app),
        Section::Keybinds => keybinds_section(app),
        Section::About => about_section(),
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_t| iced::widget::container::Style {
        background: Some(iced::Background::Color(Color::from_rgba8(0, 0, 0, 0.25))),
        border: iced::Border {
            color: theme::BORDER,
            width: 1.0,
            radius: iced::border::Radius::from(10.0),
        },
        ..Default::default()
    });

    let main = container(
        iced::widget::row![nav, content]
            .spacing(16.0)
            .width(Length::Fill),
    )
    .width(iced::Pixels(880.0))
    .height(Length::Fill)
    .max_height(560.0)
    .center_x(Length::Fill);

    // Shell dropdown or color picker modal overlay (click outside to dismiss).
    // The panel sits ABOVE the shield in a stack, so clicks inside it never
    // reach the close handler.
    let shell_dropdown_active = app.settings_shell_menu_open || app.shell_menu_closing;
    if shell_dropdown_active || app.picker.is_some() {
        // Build overlays inside a stack constrained to main's dimensions
        // so positioning anchors are relative to the settings panel, not the window.
        let mut inner_stack = iced::widget::stack![main];

        // Full-screen closer shield — catches any click outside open menus.
        // Only active while the menu is OPEN (not during close animation).
        if app.settings_shell_menu_open {
            inner_stack = inner_stack.push(
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .on_press(Message::SettingsShellMenuClose),
            );
        }
        if app.picker.is_some() {
            inner_stack = inner_stack.push(
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .on_press(Message::GlowPickerClose),
            );
        }

        // Shell dropdown overlay (visible during both open and close anim)
        if shell_dropdown_active {
            let p = if app.settings_shell_menu_open {
                crate::animation::menu_ease(
                    (app.shell_menu_anim_t / crate::animation::MENU_ANIM_MS).clamp(0.0, 1.0),
                )
            } else {
                1.0 - crate::animation::menu_close_ease(
                    1.0 - (app.shell_menu_anim_t / crate::animation::MENU_ANIM_MS).clamp(0.0, 1.0),
                )
            };
            // The animated panel is a fixed-size box that the menu is clipped
            // into, so the box must be the menu's *natural* size — a taller
            // one keeps painting an empty strip past the last option.
            let e_w = (SHELL_MENU_W * p).max(1.0);
            let e_h = (SHELL_MENU_H * p).max(1.0);
            let menu_panel = container(shell_menu(app)).width(Pixels(SHELL_MENU_W));
            let animated_menu = container(menu_panel)
                .width(Pixels(e_w))
                .height(Pixels(e_h))
                .clip(true)
                .style(|_t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(theme::BG_PRIMARY)),
                    border: iced::Border {
                        color: theme::BORDER,
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                });
            inner_stack = inner_stack.push(
                container(animated_menu)
                    .padding(crate::styles::pad4(88.0, 16.0, 0.0, 0.0))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Right)
                    .align_y(alignment::Vertical::Top),
            );
        }

        // Color picker overlay
        if app.picker.is_some() {
            let p = crate::animation::menu_ease(
                (app.picker_anim_t / crate::animation::MENU_ANIM_MS).clamp(0.0, 1.0),
            );
            let panel = picker_overlay(app);
            let panel = crate::animation::Shifted::new(
                iced::Vector::new(0.0, (1.0 - p) * -10.0),
                panel,
            );
            inner_stack = inner_stack.push(
                container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            );
        }

        container(inner_stack)
            .width(iced::Pixels(880.0))
            .height(Length::Fill)
            .max_height(560.0)
            .center_x(Length::Fill)
            .into()
    } else {
        main.into()
    }
}

fn nav_item(active: Section, s: Section) -> iced::Element<'static, Message> {
    let is_active = active == s;
    button(
        container(
            iced::widget::row![
                icon(s.icon(), if is_active { theme::TEXT } else { theme::DIM }, 13.0, 2.0),
                text(s.label())
                    .size(12.0)
                    .color(if is_active { theme::TEXT } else { theme::DIM }),
            ]
            .spacing(8.0),
        )
        .width(Length::Fill)
        .padding([8.0, 10.0]),
    )
    .width(Length::Fill)
    .on_press(Message::SettingsSection(s))
    .style(move |_t, st| iced::widget::button::Style {
        background: Some(iced::Background::Color(if is_active {
            theme::BG_ELEVATED
        } else if matches!(st, button::Status::Hovered | button::Status::Pressed) {
            theme::HOVER
        } else {
            iced::Color::TRANSPARENT
        })),
        border: iced::Border {
            radius: iced::border::Radius::from(6.0),
            ..Default::default()
        },
        ..iced::widget::button::Style::default()
    })
    .into()
}

/// A standard settings row: label left, control right, 44px tall.
fn setting_row(
    label: &str,
    control: iced::Element<'static, Message>,
) -> iced::Element<'static, Message> {
    container(
        iced::widget::row![
            text(label.to_string()).size(13.0).color(theme::TEXT),
            iced::widget::Space::new().width(Length::Fill),
            control,
        ]
        .spacing(12.0)
        .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .height(Pixels(44.0))
    .align_y(alignment::Vertical::Center)
    .into()
}

fn section_title(t: &str) -> iced::Element<'static, Message> {
    container(text(t.to_string()).size(14.0).color(theme::TEXT))
        .width(Length::Fill)
        .padding(crate::styles::pad4(4.0, 0.0, 10.0, 0.0))
        .into()
}

// =============================================================================
// Appearance
// =============================================================================

const SHELL_OPTIONS: [&str; 5] = ["系统默认", "bash", "zsh", "fish", "sh"];

fn appearance_section(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let statusbar = toggle_switch(
        app.toggle_progress[0],
        Message::TermToggleStatusbar,
    );
    let vertical = toggle_switch(
        app.toggle_progress[1],
        Message::TermToggleTabsVertical,
    );

    container(
        iced::widget::column![
            section_title("外观"),
            setting_row("显示状态栏", statusbar),
            setting_row("垂直标签页", vertical),
        ]
        .spacing(2.0)
        .width(Length::Fill),
    )
    .padding(16.0)
    .into()
}

/// Shell dropdown trigger, shared by the Appearance and Shell sections.
pub fn shell_selector_trigger(app: &TerminalPanel, msg: Message) -> iced::Element<'static, Message> {
    let cur_label: String = match app.term_default_shell.as_str() {
        "" => "系统默认".to_string(),
        v => v.to_string(),
    };
    button(
        container(
            iced::widget::row![
                text(cur_label).size(12.0).color(theme::TEXT),
                icon(Icon::ChevronDown, theme::DIM, 11.0, 2.0),
            ]
            .spacing(6.0),
        )
        .padding([4.0, 10.0]),
    )
    .on_press(msg)
    .style(|_t, st| iced::widget::button::Style {
        background: Some(iced::Background::Color(match st {
            button::Status::Hovered | button::Status::Pressed => theme::BG_ELEVATED,
            _ => theme::HOVER,
        })),
        border: iced::Border {
            radius: iced::border::Radius::from(6.0),
            ..Default::default()
        },
        ..iced::widget::button::Style::default()
    })
    .into()
}

/// Shell dropdown metrics — the animated panel is a fixed-size box that the
/// menu is clipped into, so its size must match the menu's natural size,
/// exactly like [`crate::terminal_panel`] does for its context menus.
///
/// iced lays text out at 1.3× the font size by default (`LineHeight`), so one
/// option row is `12 * 1.3 + 7 * 2` px tall; the whole panel is the rows plus
/// the list's padding. Deriving it here keeps the box in step with the items
/// instead of being a hand-tuned magic number.
const SHELL_MENU_W: f32 = 160.0;
const SHELL_MENU_PAD: f32 = 4.0;
const SHELL_ITEM_TEXT: f32 = 12.0;
const SHELL_ITEM_PAD_Y: f32 = 7.0;
const SHELL_ITEM_H: f32 = SHELL_ITEM_TEXT * 1.3 + SHELL_ITEM_PAD_Y * 2.0;
const SHELL_MENU_H: f32 = SHELL_OPTIONS.len() as f32 * SHELL_ITEM_H + SHELL_MENU_PAD * 2.0;

/// k-select style dropdown for the shell preference.
fn shell_menu(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let mut items = iced::widget::column![]
        .spacing(0.0)
        .padding([SHELL_MENU_PAD, SHELL_MENU_PAD]);
    for opt in SHELL_OPTIONS {
        let selected = match app.term_default_shell.as_str() {
            "" => opt == "系统默认",
            v => v == opt,
        };
        items = items.push(
            button(
                text(opt)
                    .size(SHELL_ITEM_TEXT)
                    .color(theme::TEXT),
            )
            .width(Length::Fill)
            .padding([SHELL_ITEM_PAD_Y, 10.0])
            .on_press(Message::ShellSelect(opt.to_string()))
            .style(move |_t, st| iced::widget::button::Style {
                background: Some(iced::Background::Color(
                    if selected
                        || matches!(st, button::Status::Hovered | button::Status::Pressed)
                    {
                        theme::BG_ELEVATED
                    } else {
                        Color::TRANSPARENT
                    },
                )),
                border: iced::Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(4.0),
                },
                ..iced::widget::button::Style::default()
            }),
        );
    }
    container(items)
        .width(Pixels(SHELL_MENU_W - SHELL_MENU_PAD * 2.0))
        .into()
}

/// iOS-style toggle switch with slide animation (progress 0..1), matching
/// the reference implementation in Kortina.ICED.
fn toggle_switch(progress: f32, msg: Message) -> iced::Element<'static, Message> {
    let t = progress.clamp(0.0, 1.0);
    let bg_color = lerp_color(theme::BG_ELEVATED, theme::BLUE, t);
    let knob_color = lerp_color(theme::TEXT, theme::BG_PRIMARY, t);
    let border_color = lerp_color(theme::BORDER, theme::BLUE, t);
    let knob_x = 2.0 + t * 20.0;

    let knob = container(iced::widget::Space::new())
        .width(Pixels(18.0))
        .height(Pixels(18.0))
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(knob_color)),
            border: iced::Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: iced::border::Radius::from(9.0),
            },
            ..Default::default()
        });

    let knob_row = iced::widget::row![iced::widget::Space::new().width(Pixels(knob_x)), knob]
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(alignment::Vertical::Center);

    let switch = container(knob_row)
        .width(Pixels(44.0))
        .height(Pixels(24.0))
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg_color)),
            border: iced::Border {
                color: border_color,
                width: 1.0,
                radius: iced::border::Radius::from(12.0),
            },
            ..Default::default()
        });

    button(switch)
        .padding(0.0)
        .style(|_t, _s| button::Style::default())
        .on_press(msg)
        .into()
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

// =============================================================================
// Glow
// =============================================================================

fn glow_section(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let accent = app.accent_color();
    let top = glow_rows(
        0,
        "上方光晕",
        app.glow.blue,
        app.glow.intensity_top,
        &app.glow_hex_top,
        accent,
    );
    let bottom = glow_rows(
        1,
        "下方光晕",
        app.glow.amber,
        app.glow.intensity_bottom,
        &app.glow_hex_bottom,
        accent,
    );

    let reset = button(text("恢复默认光晕").size(12.0).color(theme::TEXT))
        .padding([5.0, 12.0])
        .on_press(Message::GlowReset)
        .style(|_t, st| iced::widget::button::Style {
            background: Some(iced::Background::Color(match st {
                button::Status::Hovered | button::Status::Pressed => theme::BG_ELEVATED,
                _ => theme::HOVER,
            })),
            border: iced::Border {
                radius: iced::border::Radius::from(6.0),
                ..Default::default()
            },
            ..iced::widget::button::Style::default()
        });

    let accent_hex = crate::glow::rgb_of(accent);
    let accent_trigger = color_trigger(accent, format!("#{accent_hex:06x}"), 2);
    let accent_note: iced::Element<'static, Message> =
        if app.accent_override == 0 {
            container(text("默认跟随上方光晕的颜色；点色块可单独指定。").size(11.0).color(theme::DIM))
                .padding(crate::styles::pad4(10.0, 2.0, 0.0, 0.0))
                .width(Length::Fill)
                .into()
        } else {
            container(
                iced::widget::row![
                    text("已单独设定 跟随光晕").size(11.0).color(theme::DIM),
                    button(text("跟随光晕").size(11.0).color(theme::TEXT))
                        .padding([4.0, 10.0])
                        .on_press(Message::AccentFollowGlow)
                        .style(|_t, st| iced::widget::button::Style {
                            background: Some(iced::Background::Color(match st {
                                button::Status::Hovered | button::Status::Pressed => {
                                    theme::BG_ELEVATED
                                }
                                _ => theme::HOVER,
                            })),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..iced::widget::button::Style::default()
                        }),
                ]
                .spacing(8.0),
            )
            .padding(crate::styles::pad4(10.0, 2.0, 0.0, 0.0))
            .width(Length::Fill)
            .into()
        };

    container(
        iced::widget::column![
            section_title("光晕"),
            top,
            bottom,
            container(reset).padding(crate::styles::pad4(10.0, 0.0, 0.0, 0.0)),
            section_title("强调色"),
            setting_row("按钮与高亮", accent_trigger),
            container(accent_note).padding(crate::styles::pad4(10.0, 0.0, 0.0, 0.0)),
        ]
        .spacing(2.0)
        .width(Length::Fill),
    )
    .padding(16.0)
    .into()
}

/// Swatch + hex label button; opens the picker for `which` (0 = top glow,
/// 1 = bottom glow, 2 = accent).
fn color_trigger(
    color: iced::Color,
    hex: String,
    which: u8,
) -> iced::Element<'static, Message> {
    button(
        container(
            iced::widget::row![
                container(iced::widget::Space::new())
                    .width(Pixels(22.0))
                    .height(Pixels(22.0))
                    .style(move |_t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(color)),
                        border: iced::Border {
                            color: theme::BORDER,
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..Default::default()
                    }),
                text(hex).size(12.0).color(theme::DIM),
            ]
            .spacing(8.0)
            .align_y(alignment::Vertical::Center),
        )
        .padding([4.0, 8.0]),
    )
    .on_press(Message::GlowPickerOpen(which))
    .style(|_t, st| iced::widget::button::Style {
        background: Some(iced::Background::Color(match st {
            button::Status::Hovered | button::Status::Pressed => theme::BG_ELEVATED,
            _ => theme::HOVER,
        })),
        border: iced::Border {
            radius: iced::border::Radius::from(6.0),
            ..Default::default()
        },
        ..iced::widget::button::Style::default()
    })
    .into()
}

/// Color trigger row + brightness row for one glow.
fn glow_rows(
    which: u8,
    label: &str,
    color: iced::Color,
    intensity: f32,
    _hex_draft: &str,
    accent: iced::Color,
) -> iced::Element<'static, Message> {
    let hex = crate::glow::rgb_of(color);
    let trigger = color_trigger(color, format!("#{hex:06x}"), which);

    // Brightness: styled continuous slider + numeric input + wheel ±1.
    let pct = (intensity * 100.0).round();
    let slider = iced::widget::mouse_area(
        iced::widget::slider(0.0..=100.0, pct, move |v| {
            Message::GlowIntensitySet(which, v)
        })
        .step(1.0_f32)
        .width(Pixels(150.0))
        .style(move |_t, _status| iced::widget::slider::Style {
            rail: iced::widget::slider::Rail {
                backgrounds: (
                    iced::Background::Color(accent),
                    iced::Background::Color(theme::BG_ELEVATED),
                ),
                width: 6.0,
                border: Default::default(),
            },
            handle: iced::widget::slider::Handle {
                shape: iced::widget::slider::HandleShape::Circle { radius: 9.0 },
                background: iced::Background::Color(theme::TEXT),
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
            },
        }),
    )
    .on_scroll(move |d| {
        let dy = match d {
            iced::mouse::ScrollDelta::Lines { y, .. } => y,
            iced::mouse::ScrollDelta::Pixels { y, .. } => y / 20.0,
        };
        Message::GlowIntensitySet(which, pct - dy)
    });

    let num = text_input("", &format!("{}", pct as i32))
        .size(12.0)
        .width(Pixels(52.0))
        .on_input(move |s| {
            let v = s.parse::<f32>().unwrap_or(pct);
            Message::GlowIntensitySet(which, v.clamp(0.0, 100.0))
        });

    iced::widget::column![
        setting_row(
            label,
            trigger,
        ),
        setting_row("亮度", iced::widget::row![slider, num].spacing(10.0).align_y(alignment::Vertical::Center).into()),
    ]
    .spacing(0.0)
    .into()
}

// =============================================================================
// Color picker popover
// =============================================================================

fn picker_overlay(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let accent = app.accent_color();
    let which = app.picker.unwrap_or(0);
    let (h, s, v) = app.picker_hsv;
    let color = hsv_to_rgb(h, s, v);

    // SV field: canvas handles its own press/drag/release events.
    let sv = iced::widget::canvas(picker_canvas::SvCanvas {
        hue: h,
        which,
        s: app.picker_hsv.1,
        v: app.picker_hsv.2,
    })
    .width(Pixels(216.0))
    .height(Pixels(140.0));

    // Hue bar: rainbow gradient strip.
    let hue = iced::widget::canvas(picker_canvas::HueCanvas { which })
        .width(Pixels(216.0))
        .height(Pixels(14.0));

    let mut presets = iced::widget::row![].spacing(6.0);
    for rgb in GLOW_PRESETS {
        let c = iced::Color::from_rgb8(
            ((rgb >> 16) & 0xFF) as u8,
            ((rgb >> 8) & 0xFF) as u8,
            (rgb & 0xFF) as u8,
        );
        presets = presets.push(
            button(
                container(iced::widget::Space::new())
                    .width(Pixels(20.0))
                    .height(Pixels(20.0))
                    .style(move |_t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(c)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(5.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            )
            .width(Pixels(20.0))
            .height(Pixels(20.0))
            .padding(0.0)
            .on_press(Message::GlowPick(which, rgb))
            .style(|_t, _s| button::Style::default()),
        );
    }

    let draft = if which == 0 {
        &app.glow_hex_top
    } else {
        &app.glow_hex_bottom
    };

    let panel = container(
        iced::widget::column![
            text("选择颜色").size(12.0).color(theme::TEXT),
            sv,
            hue,
            presets,
            iced::widget::row![
                container(iced::widget::Space::new())
                    .width(Pixels(22.0))
                    .height(Pixels(22.0))
                    .style(move |_t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(color)),
                        border: iced::Border {
                            color: theme::BORDER,
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..Default::default()
                    }),
                text("#").size(12.0).color(theme::DIM),
                text_input("RRGGBB", draft)
                    .size(12.0)
                    .width(Pixels(84.0))
                    .on_input(move |s| Message::GlowHexInput(which, s))
                    .on_submit(Message::GlowHexSubmit(which)),
                iced::widget::Space::new().width(Length::Fill),
                button(text("应用").size(12.0).color(theme::TEXT))
                    .padding([5.0, 14.0])
                    .on_press(Message::GlowPickerApply)
                    .style(move |_t, st| iced::widget::button::Style {
                        background: Some(iced::Background::Color(match st {
                            button::Status::Hovered | button::Status::Pressed => accent,
                            _ => theme::BG_ELEVATED,
                        })),
                        border: iced::Border {
                            color: theme::BORDER,
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..iced::widget::button::Style::default()
                    }),
            ]
            .spacing(8.0)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(12.0),
    )
    .padding(16.0)
    .width(Pixels(260.0))
    .style(|_t| iced::widget::container::Style {
        background: Some(iced::Background::Color(theme::BG_PRIMARY)),
        border: iced::Border {
            color: theme::BORDER,
            width: 1.0,
            radius: iced::border::Radius::from(10.0),
        },
        ..Default::default()
    });

    // Click-outside handling now lives in the stack layout (see
    // `settings_panel`); the panel itself is returned bare.
    panel.into()
}

/// HSV → RGB (`h` in degrees, `s`/`v` in 0..1).
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> iced::Color {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    iced::Color { r: r1 + m, g: g1 + m, b: b1 + m, a: 1.0 }
}

/// RGB → HSV (`h` in degrees, `s`/`v` in 0..1).
pub fn rgb_to_hsv(c: iced::Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

// =============================================================================
// Keybinds
// =============================================================================

fn keybinds_section(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let accent = app.accent_color();
    let mut rows = iced::widget::column![].spacing(0.0);
    // Detect conflicts (same combo bound to 2+ actions) to flag them red.
    for (action, combo) in &app.keybinds {
        let capturing = app.keybind_capture == Some(*action);
        let conflict = app
            .keybinds
            .iter()
            .filter(|(_, c)| c == combo)
            .count()
            > 1;
        let control: iced::Element<'static, Message> = if capturing {
            // Recording state: dashed slot with a prompt.
            container(
                text("按下新的快捷键… (Esc 取消)")
                    .size(12.0)
                    .color(accent),
            )
            .padding([5.0, 12.0])
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::BG_ELEVATED)),
                border: iced::Border {
                    color: accent,
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            })
            .into()
        } else {
            // Static state: kbd caps inside a real bordered button.
            button(kbd_caps(combo, conflict))
                .padding([4.0, 8.0])
                .on_press(Message::KeyBindListen(*action))
                .style(|_t, st| iced::widget::button::Style {
                    background: Some(iced::Background::Color(match st {
                        button::Status::Hovered | button::Status::Pressed => theme::BG_ELEVATED,
                        _ => theme::HOVER,
                    })),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..iced::widget::button::Style::default()
                })
                .into()
        };

        let mut label_row = iced::widget::row![
            text(action.label()).size(13.0).color(theme::TEXT),
            iced::widget::Space::new().width(Length::Fill),
            control,
        ]
        .spacing(12.0)
        .align_y(alignment::Vertical::Center);
        if conflict && !capturing {
            label_row = label_row.push(
                text("冲突")
                    .size(10.0)
                    .color(iced::Color::from_rgb8(0xe5, 0x48, 0x4d)),
            );
        }
        rows = rows.push(
            container(label_row)
                .width(Length::Fill)
                .height(Pixels(44.0))
                .align_y(alignment::Vertical::Center),
        );
    }

    container(
        iced::widget::column![section_title("快捷键"), rows]
            .spacing(2.0)
            .width(Length::Fill),
    )
    .padding(16.0)
    .into()
}

/// Kbd-style caps: each modifier/key is its own bordered square, joined
/// by `+` separators.
fn kbd_caps(combo: &str, conflict: bool) -> iced::Element<'static, Message> {
    let mut row = iced::widget::row![].spacing(4.0).align_y(alignment::Vertical::Center);
    let parts: Vec<&str> = combo.split('+').collect();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            row = row.push(text("+").size(10.0).color(theme::DIM));
        }
        let label = match *p {
            "ctrl" => "Ctrl",
            "shift" => "Shift",
            "alt" => "Alt",
            "right" => "→",
            "left" => "←",
            "up" => "↑",
            "down" => "↓",
            "enter" => "⏎",
            other => &other.to_uppercase(),
        };
        row = row.push(
            container(text(label.to_string()).size(10.0).color(if conflict {
                iced::Color::from_rgb8(0xe5, 0x48, 0x4d)
            } else {
                theme::TEXT
            }))
            .padding([2.0, 6.0])
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::BG_PRIMARY)),
                border: iced::Border {
                    color: if conflict {
                        iced::Color::from_rgb8(0xe5, 0x48, 0x4d)
                    } else {
                        theme::BORDER
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(4.0),
                },
                ..Default::default()
            }),
        );
    }
    row.into()
}

// =============================================================================
// About
// =============================================================================

fn about_section() -> iced::Element<'static, Message> {
    container(
        iced::widget::column![
            section_title("关于"),
            setting_row("应用", text("Korterm").size(12.0).color(theme::TEXT).into()),
            setting_row(
                "版本",
                text(env!("CARGO_PKG_VERSION")).size(12.0).color(theme::DIM).into()
            ),
            setting_row(
                "快捷终端",
                text("绑定系统快捷键到 `korterm --quick`").size(12.0).color(theme::DIM).into()
            ),
        ]
        .spacing(2.0)
        .width(Length::Fill),
    )
    .padding(16.0)
    .into()
}

// =============================================================================
// Shell integration
// =============================================================================

/// Small filled button used by the Shell section (matches the rest of
/// the settings page: flat surface, rounded, hover lift).
pub fn action_button(label: &str, msg: Message, primary: bool) -> iced::Element<'static, Message> {
    action_button_with(label, msg, primary, None)
}

/// Same as [`action_button`] but lets the caller supply the accent color
/// (used where the panel's accent color is available).
pub fn action_button_with(
    label: &str,
    msg: Message,
    primary: bool,
    accent: Option<iced::Color>,
) -> iced::Element<'static, Message> {
    button(
        text(label.to_string())
            .size(12.0)
            .color(if primary { theme::BG_PRIMARY } else { theme::TEXT }),
    )
    .padding([7.0, 14.0])
    .on_press(msg)
    .style(move |_t, st| iced::widget::button::Style {
        background: Some(iced::Background::Color(if primary {
            accent.unwrap_or(theme::BLUE)
        } else {
            match st {
                button::Status::Hovered | button::Status::Pressed => theme::BG_ELEVATED,
                _ => Color::TRANSPARENT,
            }
        })),
        border: iced::Border {
            color: if primary {
                Color::TRANSPARENT
            } else {
                theme::BORDER
            },
            width: if primary { 0.0 } else { 1.0 },
            radius: iced::border::Radius::from(6.0),
        },
        ..Default::default()
    })
    .into()
}

fn support_label(s: crate::shell_integration::Support) -> iced::Element<'static, Message> {
    let color = if s == crate::shell_integration::Support::Missing {
        theme::DIM
    } else if s == crate::shell_integration::Support::Enabled {
        theme::TEXT
    } else {
        theme::DIM
    };
    text(s.label().to_string()).size(12.0).color(color).into()
}

fn shell_section(app: &TerminalPanel) -> iced::Element<'static, Message> {
    let st = &app.shell_status;

    let mut rows = iced::widget::column![]
        .spacing(2.0)
        .width(Length::Fill);

    rows = rows.push(section_title("Shell"));

    rows = rows.push(setting_row(
        "当前 Shell",
        shell_selector_trigger(app, Message::SettingsShellMenuToggle),
    ));
    rows = rows.push(
        container(
            text(st.shell_display.clone())
                .size(11.0)
                .color(theme::DIM),
        )
        .padding(crate::styles::pad4(0.0, 0.0, 8.0, 0.0))
        .width(Length::Fill),
    );

    if st.family == crate::shell_integration::Family::Fish {
        rows = rows.push(setting_row(
            "语法高亮 / 补全",
            text("fish 自带，无需配置")
                .size(12.0)
                .color(theme::DIM)
                .into(),
        ));
    } else if !st.family.is_supported() {
        rows = rows.push(setting_row(
            "集成",
            text("暂不支持该 Shell")
                .size(12.0)
                .color(theme::DIM)
                .into(),
        ));
    } else {
        rows = rows.push(setting_row("语法高亮", support_label(st.syntax)));
        if st.family == crate::shell_integration::Family::Zsh {
            rows = rows.push(setting_row("自动建议", support_label(st.suggest)));
        }
        rows = rows.push(setting_row("Tab 补全", support_label(st.completion)));

        rows = rows.push(setting_row(
            "启用集成",
            if st.enabled {
                action_button("移除", Message::ShellIntegrationDisable, false)
            } else {
                action_button("启用", Message::ShellIntegrationEnable, true)
            },
        ));

        let cmd = st.install_command();
        if !cmd.is_empty() {
            rows = rows.push(setting_row(
                "安装缺失组件",
                iced::widget::row![
                    action_button("复制命令", Message::ShellCopyInstallCmd, false),
                    action_button("粘贴到终端", Message::ShellPasteInstallCmd, false),
                    action_button_with(
                        "粘贴并执行",
                        Message::ShellRunInstallCmd,
                        true,
                        Some(app.accent_color()),
                    ),
                ]
                .spacing(8.0)
                .into(),
            ));
            rows = rows.push(
                container(
                    text(cmd.clone())
                        .size(11.0)
                        .color(theme::DIM)
                        .font(iced::Font::MONOSPACE),
                )
                .padding(crate::styles::pad4(0.0, 0.0, 8.0, 0.0))
                .width(Length::Fill),
            );
        }
    }

    if let Some(notice) = &app.shell_notice {
        rows = rows.push(
            container(
                text(notice.clone())
                    .size(11.0)
                    .color(theme::DIM),
            )
            .padding(crate::styles::pad4(0.0, 0.0, 8.0, 0.0))
            .width(Length::Fill),
        );
    }

    rows = rows.push(
        container(
            text("启用后 Korterm 会写入 ~/.config/korterm/shell-integration/ 并在你的 shell 配置末尾加载它；首次修改前会自动备份，可随时移除。启用后终端能感知命令的开始与结束，关闭标签页或窗口前会先确认。")
                .size(11.0)
                .color(theme::DIM),
        )
        .padding(crate::styles::pad4(10.0, 0.0, 0.0, 0.0))
        .width(Length::Fill),
    );

    container(rows).padding(16.0).into()
}

fn system_section(app: &TerminalPanel) -> iced::Element<'static, Message> {
    // Which backend are we actually on? winit picks X11 only when
    // WAYLAND_DISPLAY is unset, so this mirrors its own rule.
    let on_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
    let backend = if on_wayland { "Wayland" } else { "X11 (XWayland)" };

    let mut rows = iced::widget::column![]
        .spacing(2.0)
        .width(Length::Fill);
    rows = rows.push(section_title("系统"));

    rows = rows.push(setting_row(
        "窗口后端",
        text(backend.to_string()).size(12.0).color(theme::TEXT).into(),
    ));

    rows = rows.push(setting_row(
        "文件拖放",
        text(if on_wayland {
            "Wayland 下不支持"
        } else {
            "可用"
        })
        .size(12.0)
        .color(theme::DIM)
        .into(),
    ));

    rows = rows.push(setting_row(
        "切换到 X11",
        if app.x11_backend {
            action_button("恢复 Wayland", Message::BackendUseWayland, true)
        } else {
            action_button("启用 X11 后端", Message::BackendUseX11, true)
        },
    ));

    rows = rows.push(
        container(
            text("文件拖放依赖 X11 后端（底层窗口库尚未实现 Wayland 拖放协议）。切换后需要重启 Korterm 生效；也可以用 korterm --x11 只为这一次启动启用。")
                .size(11.0)
                .color(theme::DIM),
        )
        .padding(crate::styles::pad4(10.0, 0.0, 0.0, 0.0))
        .width(Length::Fill),
    );

    container(rows).padding(16.0).into()
}

// =============================================================================
// Canvas: SV field + hue strip
// =============================================================================

mod picker_canvas {
    use iced::widget::canvas;
    use iced::widget::canvas::Action;
    use iced::advanced::mouse;
    use iced::{Color, Point, Rectangle};
    

    use super::hsv_to_rgb;
    use crate::terminal_panel::Message;

    /// Shared per-widget state: whether the pointer is pressed inside.
    #[derive(Default)]
    pub struct DragState {
        pressed: bool,
    }

    fn gradient(start: Point, end: Point, stops: [(f32, Color); 2]) -> canvas::Fill {
        let mut g = canvas::gradient::Linear::new(start, end);
        for (o, c) in stops {
            g = g.add_stop(o, c);
        }
        canvas::Fill::from(g)
    }

    /// Saturation (x) / value (y) field at a fixed hue. Handles its own
    /// press/drag events and publishes color updates.
    pub struct SvCanvas {
        pub hue: f32,
        pub which: u8,
        pub s: f32,
        pub v: f32,
    }

    impl canvas::Program<Message> for SvCanvas {
        type State = DragState;

        fn update(
            &self,
            state: &mut Self::State,
            event: &iced::Event,
            bounds: Rectangle,
            cursor: mouse::Cursor,
        ) -> Option<Action<Message>> {
            let pos = cursor.position_in(bounds);
            match event {
                iced::Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                    state.pressed = pos.is_some();
                    pos.map(|p| {
                        let s = (p.x / bounds.width).clamp(0.0, 1.0);
                        let v = 1.0 - (p.y / bounds.height).clamp(0.0, 1.0);
                        Action::publish(Message::GlowSvDrag(self.which, s, v))
                    })
                }
                iced::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.pressed => {
                    pos.map(|p| {
                        let s = (p.x / bounds.width).clamp(0.0, 1.0);
                        let v = 1.0 - (p.y / bounds.height).clamp(0.0, 1.0);
                        Action::publish(Message::GlowSvDrag(self.which, s, v))
                    })
                }
                iced::Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                    state.pressed = false;
                    None
                }
                _ => None,
            }
        }

        fn mouse_interaction(
            &self,
            _state: &Self::State,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> mouse::Interaction {
            mouse::Interaction::Crosshair
        }

        fn draw(
            &self,
            _state: &Self::State,
            renderer: &iced::Renderer,
            _theme: &iced::Theme,
            bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Vec<canvas::Geometry> {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            let size = bounds.size();
            // Hue base, then white→transparent along +x (saturation), then
            // transparent→black along -y (value) — true gradients, matching
            // how the background glows are painted.
            frame.fill_rectangle(Point::ORIGIN, size, hsv_to_rgb(self.hue, 1.0, 1.0));
            frame.fill_rectangle(
                Point::ORIGIN,
                size,
                gradient(
                    Point::ORIGIN,
                    Point::new(size.width, 0.0),
                    [
                        (0.0, Color::from_rgba8(255, 255, 255, 1.0)),
                        (1.0, Color::from_rgba8(255, 255, 255, 0.0)),
                    ],
                ),
            );
            frame.fill_rectangle(
                Point::ORIGIN,
                size,
                gradient(
                    Point::new(0.0, size.height),
                    Point::ORIGIN,
                    [
                        (0.0, Color::from_rgba8(0, 0, 0, 1.0)),
                        (1.0, Color::from_rgba8(0, 0, 0, 0.0)),
                    ],
                ),
            );
            // Draggable selection dot.
            let px = Point::new(self.s * size.width, (1.0 - self.v) * size.height);
            frame.fill(&canvas::Path::circle(px, 6.0), Color::TRANSPARENT);
            frame.stroke(
                &canvas::Path::circle(px, 6.0),
                canvas::Stroke::default()
                    .with_color(Color::WHITE)
                    .with_width(2.0),
            );
            vec![frame.into_geometry()]
        }
    }

    /// Horizontal hue strip (rainbow gradient), press/drag to pick.
    pub struct HueCanvas {
        pub which: u8,
    }

    impl canvas::Program<Message> for HueCanvas {
        type State = DragState;

        fn update(
            &self,
            state: &mut Self::State,
            event: &iced::Event,
            bounds: Rectangle,
            cursor: mouse::Cursor,
        ) -> Option<Action<Message>> {
            let pos = cursor.position_in(bounds);
            match event {
                iced::Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                    state.pressed = pos.is_some();
                    pos.map(|p| {
                        let h = (p.x / bounds.width).clamp(0.0, 1.0) * 360.0;
                        Action::publish(Message::GlowHueDrag(self.which, h))
                    })
                }
                iced::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.pressed => {
                    pos.map(|p| {
                        let h = (p.x / bounds.width).clamp(0.0, 1.0) * 360.0;
                        Action::publish(Message::GlowHueDrag(self.which, h))
                    })
                }
                iced::Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                    state.pressed = false;
                    None
                }
                _ => None,
            }
        }

        fn mouse_interaction(
            &self,
            _state: &Self::State,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> mouse::Interaction {
            mouse::Interaction::Crosshair
        }

        fn draw(
            &self,
            _state: &Self::State,
            renderer: &iced::Renderer,
            _theme: &iced::Theme,
            bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Vec<canvas::Geometry> {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            let size = bounds.size();
            let rainbow = canvas::gradient::Linear::new(Point::ORIGIN, Point::new(size.width, 0.0))
                .add_stop(0.000, Color::from_rgb8(255, 0, 0))
                .add_stop(0.167, Color::from_rgb8(255, 255, 0))
                .add_stop(0.333, Color::from_rgb8(0, 255, 0))
                .add_stop(0.500, Color::from_rgb8(0, 255, 255))
                .add_stop(0.667, Color::from_rgb8(0, 0, 255))
                .add_stop(0.833, Color::from_rgb8(255, 0, 255))
                .add_stop(1.000, Color::from_rgb8(255, 0, 0));
            frame.fill_rectangle(Point::ORIGIN, size, canvas::Fill::from(rainbow));
            vec![frame.into_geometry()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn hsv_primaries() {
        let cases = [
            (0.0_f32, 0.0_f32, 1.0_f32, (1.0, 1.0, 1.0)),  // white
            (123.0, 0.0, 0.0, (0.0, 0.0, 0.0)),            // black (hue ignored)
            (0.0, 1.0, 1.0, (1.0, 0.0, 0.0)),              // red
            (120.0, 1.0, 1.0, (0.0, 1.0, 0.0)),            // green
            (240.0, 1.0, 1.0, (0.0, 0.0, 1.0)),            // blue
        ];
        for (h, s, v, (r, g, b)) in cases {
            let c = hsv_to_rgb(h, s, v);
            assert!(close(c.r, r) && close(c.g, g) && close(c.b, b), "hsv({h},{s},{v})");
        }
    }

    #[test]
    fn hsv_rgb_roundtrip() {
        for h in (0..360).step_by(30) {
            for s in [0.3_f32, 0.7, 1.0] {
                for v in [0.3_f32, 0.7, 1.0] {
                    let rgb = hsv_to_rgb(h as f32, s, v);
                    let (h2, s2, v2) = rgb_to_hsv(rgb);
                    let hue_err = (h2 - h as f32).rem_euclid(360.0);
                    assert!(
                        close(hue_err.min(360.0 - hue_err), 0.0),
                        "h {h} -> {h2}"
                    );
                    assert!(close(s2, s), "s {s} -> {s2}");
                    assert!(close(v2, v), "v {v} -> {v2}");
                }
            }
        }
    }
}
