// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Custom titlebar — 44px with traffic lights and integrated tabs.

use iced::widget::{button, container, mouse_area, row, scrollable, text};
use iced::{Color, Length, Pixels};

use crate::icons::Icon;
use crate::styles;
use crate::theme;
use crate::terminal_panel::{Message, TerminalPanel};

pub const HEIGHT: f32 = 44.0;

fn icon_btn(icon: Icon, msg: Option<Message>) -> button::Button<'static, Message> {
    let content = container(crate::icons::icon(icon, theme::DIM, 18.0, 1.5))
        .width(Pixels(30.0))
        .height(Pixels(30.0))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center);

    let btn = button(content)
        .width(Pixels(30.0))
        .height(Pixels(30.0))
        .padding(0.0)
        .style(styles::icon_button(theme::DIM));

    match msg {
        Some(m) => btn.on_press(m),
        None => btn,
    }
}

fn traffic_light(
    base: Color,
    press_msg: Message,
) -> iced::Element<'static, Message> {
    const SIZE: f32 = 16.0;
    let circle = container(iced::widget::Space::new())
        .width(Pixels(SIZE))
        .height(Pixels(SIZE))
        .style(move |_t: &iced::Theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(base)),
            border: iced::Border {
                color: theme::LIGHT_BORDER,
                width: 0.5,
                radius: iced::border::Radius::from(SIZE / 2.0),
            },
            ..iced::widget::container::Style::default()
        });

    let btn = button(circle)
        .width(Pixels(SIZE))
        .height(Pixels(SIZE))
        .padding(0.0)
        .style(|_t, _s| button::Style {
            background: Some(iced::Background::Color(Color::TRANSPARENT)),
            ..button::Style::default()
        })
        .on_press(press_msg);

    mouse_area(btn).into()
}

/// A draggable tab. Uses `mouse_area` instead of a `button` so that
/// press → move → release is fully observable: press starts a potential
/// drag, moving across another tab reorders live (`TermTabHover`), and
/// release selects the tab unless it was actually reordered. The close ✕
/// stays a real button — it captures its own press/release, so clicking it
/// never starts a drag or selects the tab.
// Widget builders legitimately take the whole visual state as parameters.
#[allow(clippy::too_many_arguments)]
fn term_tab(
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
    .align_y(iced::alignment::Vertical::Center);

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

    // Lift: the dragged tab shifts up a couple of px (asymmetric padding
    // keeps the 32px footprint so neighbours don't reflow).
    // Tween: FLIP slide keyed by the tab's stable id — the tab glides from
    // its own old position to the new slot on reorder.
    crate::animation::Tween::new(
        id as u64,
        tab_tweens,
        mouse_area(
            container(content)
                .padding(styles::pad4(
                    4.0 - 2.0 * lift,
                    12.0,
                    4.0 + 2.0 * lift,
                    12.0,
                ))
                .height(Pixels(32.0))
                .align_y(iced::alignment::Vertical::Center)
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

/// A vertical divider line between the traffic lights and tabs.
fn divider() -> iced::Element<'static, Message> {
    container(iced::widget::Space::new())
        .width(Pixels(1.0))
        .height(Pixels(20.0))
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BORDER)),
            ..Default::default()
        })
        .into()
}

pub fn view(app: &TerminalPanel) -> iced::Element<'static, Message> {
    // ---- left: traffic lights + divider ----
    let lights = row![
        traffic_light(theme::LIGHT_CLOSE, Message::CloseWindow),
        traffic_light(theme::LIGHT_MIN, Message::MinimizeWindow),
        traffic_light(theme::LIGHT_MAX, Message::ToggleMaximize),
    ]
    .spacing(7.0)
    .padding([0.0, 4.0])
    .align_y(iced::alignment::Vertical::Center);

    let left = row![lights, divider()]
        .spacing(12.0)
        .align_y(iced::alignment::Vertical::Center);

    // ---- center: tabs (fill) or app name (fill) ----
    // Always Fill so the drag region is continuous between left and right.
    // NOTE: `Pixels` sets the *minimum* of child layout limits in iced, so
    // the scrollable and the title text are pinned to exact heights —
    // otherwise the 44px min clamps the tab row and pills sit flush at the
    // top of the titlebar instead of vertically centered.
    let center: iced::Element<'static, Message> = if app.term_tabs_vertical || app.terminals.is_empty() {
        container(
            text("Korterm")
                .size(13.0)
                .font(theme::sans())
                .color(theme::TEXT),
        )
        .width(Length::Fill)
        .height(Pixels(HEIGHT))
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Center)
        .into()
    } else {
        let mut tabs = row![].spacing(4.0);
        for (idx, term) in app.terminals.iter().enumerate() {
            let active = app.active_terminal == Some(idx);
            let hovered = app.tab_hover == Some(idx);
            let dragging = app.term_tab_drag.is_some_and(|d| d.idx == idx);
            let lift = if dragging { crate::terminal_panel::tab_lift(app) } else { 0.0 };
            let pulse = crate::terminal_panel::tab_pulse(app, idx);
            tabs = tabs.push(term_tab(
                term.id,
                term.display_title(idx),
                active,
                idx,
                hovered,
                lift,
                pulse,
                &app.tab_tweens,
            ));
        }
        // mouse_area(on_scroll) sits outside the scrollable: plain vertical
        // wheel is *not* consumed by a horizontal scrollable (Status stays
        // Ignored), so we get it here and scroll the strip manually.
        mouse_area(
            scrollable(
                container(tabs)
                    .width(Length::Shrink)
                    .height(Pixels(32.0))
                    .align_y(iced::alignment::Vertical::Center),
            )
            .id(iced::widget::Id::new("titlebar-tabs"))
            .width(Length::Fill)
            .height(Pixels(32.0))
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(0.0).scroller_width(0.0),
            ))
            .on_scroll(|vp| {
                Message::TitlebarScrolled(vp.absolute_offset().x)
            }),
        )
        .on_scroll(Message::TitlebarScroll)
        .into()
    };

    // ---- right: minimal actions ----
    let right = row![
        icon_btn(Icon::Plus, Some(Message::TermNew)),
        icon_btn(
            if app.term_tabs_vertical { Icon::PanelTop } else { Icon::PanelRight },
            Some(Message::TermToggleTabsVertical),
        ),
        icon_btn(Icon::ChevronDown, Some(Message::TermShowShellSelector)),
        icon_btn(Icon::LayoutGrid, Some(Message::TermShowActionsMenu)),
    ]
    .spacing(4.0)
    .align_y(iced::alignment::Vertical::Center);

    // ---- assemble: left (shrink) | center (fill, drag) | right (shrink) ----
    // Center must be wrapped in a Fill container so it actually expands.
    // The mouse_area is applied to that container so the entire middle
    // band is a drag region. Buttons inside left/right capture their own
    // presses via Iced's event bubbling.
    let center_drag = container(center)
        .width(Length::Fill)
        .height(Pixels(HEIGHT))
        .align_y(iced::alignment::Vertical::Center);

    let bar = row![
        left,
        mouse_area(center_drag).on_press(Message::DragTitlebar),
        right,
    ]
    .width(Length::Fill)
    .height(Pixels(HEIGHT))
    .align_y(iced::alignment::Vertical::Center);

    let bar_elem: iced::Element<'static, Message> = container(bar)
        .width(Length::Fill)
        .height(Pixels(HEIGHT))
        .padding([0.0, 12.0])
        .align_y(iced::alignment::Vertical::Center)
        .into();

    // Hover tracking for the overflow drag-strip reveal (enter/exit are
    // not consumed by children, so buttons and tabs keep working). The x
    // coordinate lets the app exclude the corner button zones.
    mouse_area(bar_elem)
        .on_move(|p| Message::TitlebarHover(Some(p.x)))
        .on_exit(Message::TitlebarHover(None))
        .into()
}

/// The drag-strip zone above the titlebar, rendered at the current
/// (animated) window-extra height. Only rendered while tabs overflow and
/// the zone has height.
pub fn drag_strip(app: &TerminalPanel, zone_h: f32) -> iced::Element<'static, Message> {
    let grip_hover = app.titlebar_grip_hover;
    let inner_h = zone_h.max(0.0);

    let grip: iced::Element<'static, Message> = if grip_hover {
        container(iced::widget::Space::new())
            .width(Pixels(64.0))
            .height(Pixels(4.0))
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    theme::rgba(0xcf, 0xd1, 0xd4, 0.30),
                )),
                border: iced::Border {
                    radius: iced::border::Radius::from(2.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    } else {
        iced::widget::Space::new().into()
    };

    // Inner interactive area — grows from the top edge of the zone.
    let area = mouse_area(
        container(grip)
            .width(Length::Fill)
            .height(Pixels(inner_h))
            .center_x(Length::Fill)
            .align_y(iced::alignment::Vertical::Center)
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(if grip_hover {
                    theme::rgba(0xff, 0xff, 0xff, 0.04)
                } else {
                    Color::TRANSPARENT
                })),
                ..Default::default()
            }),
    )
    .on_press(Message::DragTitlebar)
    .on_enter(Message::TitlebarGripHover(true))
    .on_exit(Message::TitlebarGripHover(false))
    .interaction(iced::mouse::Interaction::Grab);

    // Zone container — its height IS the applied window extra, so the
    // layout below always matches the actual window size.
    container(area)
        .width(Length::Fill)
        .height(Pixels(inner_h))
        .align_y(iced::alignment::Vertical::Top)
        .clip(true)
        .into()
}