//! Six visual App Skins (Native, Synthwave, Cozy, Terminal, Disco, Zen), ported from
//! Maprisma Lights (`maprisma-hue-lights`). Native leaves the desktop theme untouched.

use cosmic::iced::widget::container as iced_container;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{container, space};
use cosmic::{Element, theme};

use crate::i18n::tr;

pub const SKIN_KEY: &str = "skin";
pub const SKINS: [&str; 6] = ["native", "synthwave", "cozy", "terminal", "disco", "zen"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinStyle {
    pub accent: Color,
    pub accent_rgb: [u8; 3],
    pub border_tint: Color,
    pub bg_tint: Color,
    pub window_bg: [u8; 3],
    pub foreground: [u8; 3],
    pub border_width: f32,
}

impl SkinStyle {
    pub fn palette(&self) -> crate::look::Palette {
        crate::look::Palette {
            accent: Some(self.accent_rgb),
            background: Some(self.window_bg),
            foreground: Some(self.foreground),
            red: None,
            yellow: None,
            green: None,
        }
    }
}

pub fn normalize_skin(skin: &str) -> &'static str {
    SKINS.iter().find(|s| **s == skin).copied().unwrap_or("native")
}

/// Visual accent palette for non-native skins; `None` for `"native"` so the desktop theme stays untouched.
pub fn skin_style(skin: &str) -> Option<SkinStyle> {
    match normalize_skin(skin) {
        "synthwave" => Some(SkinStyle {
            accent: Color::from_rgb8(0xff, 0x2e, 0x93),
            accent_rgb: [0xff, 0x2e, 0x93],
            border_tint: Color::from_rgb8(0x00, 0xe5, 0xff),
            bg_tint: Color::from_rgb8(0x3a, 0x12, 0x59),
            window_bg: [0x13, 0x09, 0x24],
            foreground: [0xf4, 0xea, 0xff],
            border_width: 1.5,
        }),
        "cozy" => Some(SkinStyle {
            accent: Color::from_rgb8(0xf0, 0x9a, 0x36),
            accent_rgb: [0xf0, 0x9a, 0x36],
            border_tint: Color::from_rgb8(0xe0, 0x82, 0x2a),
            bg_tint: Color::from_rgb8(0x59, 0x30, 0x12),
            window_bg: [0x21, 0x15, 0x0e],
            foreground: [0xf5, 0xe8, 0xd6],
            border_width: 1.25,
        }),
        "terminal" => Some(SkinStyle {
            accent: Color::from_rgb8(0x2c, 0xe8, 0x68),
            accent_rgb: [0x2c, 0xe8, 0x68],
            border_tint: Color::from_rgb8(0x22, 0xc5, 0x5e),
            bg_tint: Color::from_rgb8(0x0d, 0x38, 0x1a),
            window_bg: [0x09, 0x13, 0x0b],
            foreground: [0xd2, 0xfc, 0xe0],
            border_width: 1.25,
        }),
        "disco" => Some(SkinStyle {
            accent: Color::from_rgb8(0xd9, 0x46, 0xef),
            accent_rgb: [0xd9, 0x46, 0xef],
            border_tint: Color::from_rgb8(0xfa, 0xcc, 0x15),
            bg_tint: Color::from_rgb8(0x4c, 0x16, 0x66),
            window_bg: [0x1c, 0x0c, 0x2b],
            foreground: [0xf8, 0xec, 0xff],
            border_width: 1.5,
        }),
        "zen" => Some(SkinStyle {
            accent: Color::from_rgb8(0x52, 0xb7, 0x98),
            accent_rgb: [0x52, 0xb7, 0x98],
            border_tint: Color::from_rgb8(0x64, 0xc2, 0xa4),
            bg_tint: Color::from_rgb8(0x1e, 0x38, 0x32),
            window_bg: [0x14, 0x1d, 0x1c],
            foreground: [0xe2, 0xf0, 0xec],
            border_width: 1.0,
        }),
        _ => None,
    }
}

pub fn skin_label(skin: &str) -> String {
    match normalize_skin(skin) {
        "synthwave" => tr("Synthwave"),
        "cozy" => tr("Cozy"),
        "terminal" => tr("Terminal"),
        "disco" => tr("Disco"),
        "zen" => tr("Zen"),
        _ => tr("Native"),
    }
}

pub fn with_alpha(mut color: Color, alpha: f32) -> Color {
    color.a = alpha;
    color
}

/// Active theme for the chosen skin and corners; `None` when Native on COSMIC with desktop corners.
pub fn active_theme(skin: &str, corners_choice: &str) -> Option<cosmic::Theme> {
    if let Some(st) = skin_style(skin) {
        let radius = crate::corners::radius(corners_choice, crate::corners::desktop_radius());
        Some(crate::look::theme_from(&st.palette(), Some(radius)))
    } else {
        crate::corners::theme(corners_choice)
    }
}

/// Sets the whole window's theme (and desktop text scale) for `--window` mode.
pub fn apply_window_theme<M: Send + 'static>(
    skin: &str,
    corners_choice: &str,
) -> cosmic::Task<cosmic::Action<M>> {
    let theme = active_theme(skin, corners_choice).unwrap_or_else(cosmic::theme::system_preference);
    cosmic::Task::batch([
        cosmic::command::set_theme(theme),
        cosmic::command::set_scaling_factor(crate::look::text_scale()),
    ])
}

/// Draws a standalone window (`--window`) with the active skin or corners theme.
pub fn apply_window<'a, M: 'a>(
    skin: &str,
    corners_choice: &str,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    match active_theme(skin, corners_choice) {
        Some(theme) => cosmic::iced::widget::themer(Some(theme), content)
            .background(crate::look::window_background)
            .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
            .into(),
        None => content.into(),
    }
}

/// Draws an applet popup wrapped in `popup_container` with the active skin or corners theme.
pub fn apply_popup<'a, M: 'static>(
    skin: &str,
    corners_choice: &str,
    core: &cosmic::app::Core,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    match active_theme(skin, corners_choice) {
        Some(theme) => {
            let inner: Element<'a, M> = if skin_style(skin).is_some() {
                let bg = Color::from(theme.cosmic().bg_color());
                let fg = Color::from(theme.cosmic().on_bg_color());
                container(content)
                    .class(theme::Container::custom(move |_| iced_container::Style {
                        background: Some(Background::Color(bg)),
                        text_color: Some(fg),
                        border: Border {
                            radius: crate::corners::now().radius.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }))
                    .into()
            } else {
                content.into()
            };
            cosmic::iced::widget::themer(Some(theme), core.applet.popup_container(inner))
                .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
                .into()
        }
        None => core.applet.popup_container(content).into(),
    }
}

pub fn card_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border, width) = match skin_st {
            Some(st) => (with_alpha(st.bg_tint, 0.28), with_alpha(st.border_tint, 0.48), st.border_width),
            None => (with_alpha(base, 0.05), with_alpha(base, 0.10), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: crate::corners::now().radius.into(),
                width,
                color: border,
            },
            ..Default::default()
        }
    })
}

pub fn sub_card_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border) = match skin_st {
            Some(st) => (with_alpha(st.bg_tint, 0.22), with_alpha(st.border_tint, 0.40)),
            None => (with_alpha(base, 0.04), with_alpha(base, 0.12)),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: crate::corners::now().small.into(),
                width: 1.0,
                color: border,
            },
            ..Default::default()
        }
    })
}

pub fn pill_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| match skin_st {
        Some(st) => iced_container::Style {
            background: Some(Background::Color(with_alpha(st.accent, 0.24))),
            border: Border {
                radius: crate::corners::now().pill.into(),
                width: 1.0,
                color: with_alpha(st.border_tint, 0.48),
            },
            ..Default::default()
        },
        None => iced_container::Style {
            background: Some(Background::Color(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.12))),
            border: Border {
                radius: crate::corners::now().pill.into(),
                ..Default::default()
            },
            ..Default::default()
        },
    })
}

pub fn swatch<'a, M: 'a>(color: Color, size: f32) -> Element<'a, M> {
    container(space::horizontal())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            iced_container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    radius: crate::corners::now().pill.min(size / 2.0).into(),
                    width: 1.0,
                    color: with_alpha(base, 0.3),
                },
                ..Default::default()
            }
        }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skins_normalize_and_style() {
        assert_eq!(normalize_skin(""), "native");
        assert_eq!(normalize_skin("bogus"), "native");
        assert_eq!(skin_style("native"), None);
        for skin in SKINS {
            assert_eq!(normalize_skin(skin), skin);
            assert!(!skin_label(skin).is_empty());
            if skin == "native" {
                assert!(skin_style(skin).is_none());
            } else {
                let st = skin_style(skin).expect("non-native skin should have a style");
                assert!(st.border_width >= 1.0);
            }
        }
    }

    #[test]
    fn skin_palettes_populate_theme_colors() {
        assert!(skin_style("native").is_none());
        for skin in SKINS {
            if skin == "native" {
                continue;
            }
            let st = skin_style(skin).expect("non-native skin must have SkinStyle");
            let pal = st.palette();
            assert!(!pal.is_empty(), "skin {skin} palette must not be empty");
            assert_eq!(pal.accent, Some(st.accent_rgb));
            assert_eq!(pal.background, Some(st.window_bg));
            assert_eq!(pal.foreground, Some(st.foreground));
            assert!(pal.red.is_none());
            assert!(pal.yellow.is_none());
            assert!(pal.green.is_none());
            let theme = active_theme(skin, "rounded").expect("non-native skin builds a theme");
            assert_eq!(theme.cosmic().corner_radii.radius_m, [12.0; 4]);
            assert!(theme.cosmic().is_dark);
        }
    }
}
