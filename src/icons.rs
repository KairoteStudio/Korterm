// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Icon system using vector-icons crate (GPU-rendered, anti-aliased)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    PanelRight,
    PanelTop,
    X,
    Plus,
    Square,
    Terminal,
    Trash2,
    Eye,
    EyeOff,
    ChevronDown,
    ChevronUp,
    LayoutGrid,
    Settings,
    Search,
    Info,
}

fn body(icon: Icon) -> &'static str {
    match icon {
        Icon::PanelRight => {
            r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 3v18"/>"#
        }
        Icon::PanelTop => {
            r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/>"#
        }
        Icon::X => {
            r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#
        }
        Icon::Plus => {
            r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#
        }
        Icon::Square => {
            r#"<rect width="18" height="18" x="3" y="3" rx="2"/>"#
        }
        Icon::Terminal => {
            r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#
        }
        Icon::Trash2 => {
            r#"<path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>"#
        }
        Icon::Eye => {
            r#"<path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10 7-10 7Z"/><circle cx="12" cy="12" r="3"/>"#
        }
        Icon::EyeOff => {
            r#"<path d="M9.88 9.88a3 3 0 1 0 4.24 4.24"/><path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"/><path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61"/><line x1="2" y1="2" x2="22" y2="22"/>"#
        }
        Icon::ChevronDown => {
            r#"<path d="m6 9 6 6 6-6"/>"#
        }
        Icon::ChevronUp => {
            r#"<path d="m18 15-6-6-6 6"/>"#
        }
        Icon::LayoutGrid => {
            r#"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"#
        }
        Icon::Settings => {
            r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#
        }
        Icon::Search => {
            r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#
        }
        Icon::Info => {
            r#"<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="16" y2="12"/><line x1="12" x2="12.01" y1="8" y2="8"/>"#
        }
    }
}

pub fn icon<M>(
    icon: Icon,
    color: iced::Color,
    size: f32,
    stroke: f32,
) -> iced::Element<'static, M>
where
    M: 'static,
{
    vector_icons::lucide(body(icon), color, size, stroke).into()
}