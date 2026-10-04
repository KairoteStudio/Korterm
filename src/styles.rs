// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Shared style closures — Fleet theme style system

use iced::border::{Border, Radius};
use iced::widget::{button, container, scrollable};
use iced::{Background, Color};

use crate::theme;

pub fn island() -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::BG_PANEL)),
        text_color: None,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(11.0),
        },
        shadow: Default::default(),
        snap: false,
    }
}

pub fn island_ring() -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::ISLAND_RING)),
        text_color: None,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(12.0),
        },
        shadow: Default::default(),
        snap: false,
    }
}

pub fn icon_button(color: Color) -> button::StyleFn<'static, iced::Theme> {
    Box::new(move |_theme, status| match status {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(Background::Color(theme::HOVER)),
            text_color: theme::TEXT,
            border: Border {
                radius: Radius::from(7.0),
                ..Border::default()
            },
            ..button::Style::default()
        },
        _ => button::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color: color,
            border: Border {
                radius: Radius::from(7.0),
                ..Border::default()
            },
            ..button::Style::default()
        },
    })
}

/// Pill-shaped tab background for `mouse_area`-based (draggable) tabs.
///
/// - `lift` (0..1): drag lift-in — the pill brightens toward a raised tone.
/// - `pulse` (0..1): post-drop settle flash — a brief accent tint fading out.
pub fn pill_tab_container(
    active: bool,
    hovered: bool,
    lift: f32,
    pulse: f32,
) -> container::Style {
    const DRAG_LIFT: Color = Color { r: 0.23, g: 0.24, b: 0.27, a: 1.0 }; // ~#3a3d44
    let base = if active {
        theme::BG_ELEVATED
    } else if hovered {
        theme::HOVER
    } else {
        Color::TRANSPARENT
    };
    let mut bg = theme::mix(base, DRAG_LIFT, lift);
    bg = theme::mix(bg, theme::BLUE, pulse * 0.35);
    container::Style {
        background: Some(Background::Color(bg)),
        text_color: if active || lift > 0.5 {
            Some(theme::TEXT)
        } else {
            Some(theme::DIM)
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(7.0),
        },
        ..Default::default()
    }
}

pub fn scroll_style() -> scrollable::Style {
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail(),
        horizontal_rail: rail(),
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            shadow: Default::default(),
            icon: Color::TRANSPARENT,
        },
    }
}

fn rail() -> scrollable::Rail {
    scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(Color::from_rgba8(0xff, 0xff, 0xff, 0.14)),
            border: Border {
                radius: Radius::from(3.0),
                ..Border::default()
            },
        },
    }
}

/// Four-sided padding with distinct values (needed for animated asymmetric
/// padding — `Padding` has no `From<[f32; 4]>` impl).
pub const fn pad4(top: f32, right: f32, bottom: f32, left: f32) -> iced::Padding {
    iced::Padding {
        top,
        right,
        bottom,
        left,
    }
}
/// Button style for confirmation dialogs: primary action is filled with
/// the accent colour, secondary is a plain bordered surface.
pub fn dialog_button(primary: bool) -> button::StyleFn<'static, iced::Theme> {
    Box::new(move |_t, status| button::Style {
        background: Some(Background::Color(if primary {
            crate::theme::BLUE
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => {
                    crate::theme::BG_ELEVATED
                }
                _ => Color::TRANSPARENT,
            }
        })),
        border: Border {
            color: if primary {
                Color::TRANSPARENT
            } else {
                crate::theme::BORDER
            },
            width: if primary { 0.0 } else { 1.0 },
            radius: Radius::from(6.0),
        },
        text_color: crate::theme::TEXT,
        ..Default::default()
    })
}
