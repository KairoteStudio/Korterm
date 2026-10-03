// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
mod animation;
mod config;
mod keybinds;
mod quick;
mod settings;
mod theme;
mod icons;
mod styles;
mod terminal_panel;
mod glow;
mod titlebar;

use iced::widget::{column, container, stack};
use iced::{Length, Theme, window};

use terminal_panel::{TerminalPanel, Message};

struct Korterm;

impl iced::Program for Korterm {
    type State = TerminalPanel;
    type Message = Message;
    type Theme = Theme;
    type Renderer = iced::Renderer;
    type Executor = iced::executor::Default;

    fn name() -> &'static str {
        "Korterm"
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
            size: iced::Size::new(1000.0, 700.0),
            min_size: Some(iced::Size::new(600.0, 400.0)),
            resizable: true,
            exit_on_close_request: true,
            position: window::Position::Centered,
            decorations: false,
            ..Default::default()
        })
    }

    fn boot(&self) -> (Self::State, iced::Task<Self::Message>) {
        TerminalPanel::new()
    }

    fn update(
        &self,
        state: &mut Self::State,
        message: Self::Message,
    ) -> iced::Task<Self::Message> {
        state.update(message)
    }

    fn view<'a>(
        &self,
        state: &'a Self::State,
        _window: window::Id,
    ) -> iced::Element<'a, Self::Message, Self::Theme, Self::Renderer> {
        let terminal_content = state.view();

        // Island shell: ring + island (matches Kortina exactly)
        let island = container(
            container(terminal_content)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_t| styles::island())
                .clip(true),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(1.0)
        .style(|_t| styles::island_ring())
        .clip(true);

        // Overflow drag strip: the window height animates by the applied
        // extra; the strip zone sits ABOVE the titlebar (content is never
        // squeezed). It extends after a hover delay and retracts after the
        // mouse leaves.
        let zone_h = state.strip_extra_applied;
        let mut workspace = column![];
        if state.tabs_overflow && zone_h > 0.5 {
            workspace = workspace.push(titlebar::drag_strip(state, zone_h));
        }
        workspace = workspace.push(titlebar::view(state));
        workspace = workspace.push(
            container(island)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding([8.0, 4.0]),
        );
        let workspace = workspace.width(Length::Fill).height(Length::Fill);

        stack![
            iced::widget::canvas(&state.glow).width(Length::Fill).height(Length::Fill),
            workspace,
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn title(&self, _state: &Self::State, _window: window::Id) -> String {
        String::from("Korterm")
    }

    fn subscription(
        &self,
        state: &Self::State,
    ) -> iced::Subscription<Self::Message> {
        state.subscription()
    }

    fn theme(
        &self,
        _state: &Self::State,
        _window: window::Id,
    ) -> Option<Self::Theme> {
        Some(Theme::Dark)
    }
}

fn main() -> iced::Result {
    // `--quick` runs the standalone Quake-style overlay terminal (bind a
    // system shortcut to `korterm --quick` to toggle it).
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--quick") {
        return quick::run();
    }
    iced_winit::run(Korterm)
        .map_err(iced::Error::from)
}