//! Settings › Corners, in AI Playground's three sizes. Shared copy: the same file in every
//! Maprisma COSMIC app; change it in all of them.
//!
//! "desktop" follows the desktop's window corners: the COSMIC theme's medium radius (16 Round, 8
//! Slightly round, 2 Square in COSMIC Settings), Hyprland's rounding, KDE's 8 (look.rs); "slight"
//! is 6, "rounded" 12, "square" 0. The colours come from look.rs too (the desktop's theme outside
//! COSMIC). Cards and popup containers take the radius, buttons,
//! drop-downs and fields at most 8, small parts (check boxes, tracks, badges, swatches) at most
//! 4, pills stay round (0 in Square).

use std::sync::{Arc, RwLock};

pub const CHOICES: [&str; 4] = ["desktop", "slight", "rounded", "square"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    pub radius: f32,
    pub control: f32,
    pub small: f32,
    pub pill: f32,
}

impl Sizes {
    pub fn new(radius: f32) -> Self {
        let radius = radius.max(0.0);
        Self { radius, control: radius.min(8.0), small: radius.min(4.0), pill: if radius > 0.0 { 999.0 } else { 0.0 } }
    }
}

/// An unknown or missing setting is "desktop".
pub fn normalize(setting: &str) -> &'static str {
    CHOICES.iter().find(|c| **c == setting).copied().unwrap_or("desktop")
}

pub fn radius(setting: &str, desktop: f32) -> f32 {
    match normalize(setting) {
        "slight" => 6.0,
        "rounded" => 12.0,
        "square" => 0.0,
        _ => desktop,
    }
}

/// The desktop's window corners (what "Like the desktop" follows).
pub fn desktop_radius() -> f32 {
    crate::look::window_radius().unwrap_or_else(|| cosmic::theme::active().cosmic().corner_radii.radius_m[0])
}

pub fn sizes(setting: &str) -> Sizes {
    Sizes::new(radius(setting, desktop_radius()))
}

static CURRENT: RwLock<&'static str> = RwLock::new("desktop");

/// The setting in effect, for the app's own drawing (bars, swatches, dots, badges).
pub fn set_current(setting: &str) {
    if let Ok(mut current) = CURRENT.write() {
        *current = normalize(setting);
    }
}

pub fn current() -> &'static str {
    CURRENT.read().map(|c| *c).unwrap_or("desktop")
}

/// The sizes in effect (see set_current).
pub fn now() -> Sizes {
    sizes(current())
}

/// The theme's corner radii set to these sizes, so libcosmic's own widgets (buttons, menu rows,
/// toggles, fields, the popup container) follow too.
pub fn radii(sizes: Sizes) -> cosmic::cosmic_theme::CornerRadii {
    cosmic::cosmic_theme::CornerRadii {
        radius_0: [0.0; 4],
        radius_xs: [sizes.small; 4],
        radius_s: [sizes.control; 4],
        radius_m: [sizes.radius; 4],
        radius_l: [sizes.radius; 4],
        radius_xl: [sizes.pill; 4],
    }
}

/// The desktop's theme (look.rs) with the chosen corners; None for "desktop" on COSMIC (the
/// theme libcosmic has stays as it is).
pub fn theme(setting: &str) -> Option<cosmic::Theme> {
    let desktop = crate::look::theme();
    if normalize(setting) == "desktop" && desktop.is_none() {
        return None;
    }
    let mut theme = desktop.unwrap_or_else(cosmic::theme::active);
    let mut cosmic = theme.cosmic().clone();
    cosmic.corner_radii = radii(sizes(setting));
    theme.theme_type = cosmic::theme::ThemeType::Custom(Arc::new(cosmic));
    Some(theme)
}

/// `content` drawn with the desktop's colours and the chosen corners (an applet popup: its
/// container draws the background).
pub fn apply<'a, M: 'a>(setting: &str, content: impl Into<cosmic::Element<'a, M>>) -> cosmic::Element<'a, M> {
    match theme(setting) {
        Some(theme) => cosmic::iced::widget::themer(Some(theme), content).into(),
        None => content.into(),
    }
}

/// The whole app's theme (a `--window`'s title bar too): the desktop's theme with the chosen
/// corners, else the system's. On start and whenever corners or the desktop's look change.
pub fn set_app_theme<M: Send + 'static>(setting: &str) -> cosmic::Task<cosmic::Action<M>> {
    cosmic::command::set_theme(theme(setting).unwrap_or_else(cosmic::theme::system_preference))
}

/// [apply] for a whole window (`--window`): the window's background in the theme's colour too.
pub fn apply_window<'a, M: 'a>(setting: &str, content: impl Into<cosmic::Element<'a, M>>) -> cosmic::Element<'a, M> {
    match theme(setting) {
        Some(theme) => cosmic::iced::widget::themer(Some(theme), content)
            .background(crate::look::window_background)
            .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
            .into(),
        None => content.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_sizes() {
        assert_eq!(Sizes::new(16.0), Sizes { radius: 16.0, control: 8.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(6.0), Sizes { radius: 6.0, control: 6.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(12.0), Sizes { radius: 12.0, control: 8.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(2.0), Sizes { radius: 2.0, control: 2.0, small: 2.0, pill: 999.0 });
        assert_eq!(Sizes::new(0.0), Sizes { radius: 0.0, control: 0.0, small: 0.0, pill: 0.0 });
    }

    #[test]
    fn choices() {
        assert_eq!(radius("desktop", 16.0), 16.0);
        assert_eq!(radius("slight", 16.0), 6.0);
        assert_eq!(radius("rounded", 2.0), 12.0);
        assert_eq!(radius("square", 16.0), 0.0);
        assert_eq!(radius("bogus", 8.0), 8.0);
        assert_eq!(normalize(""), "desktop");
        let r = radii(Sizes::new(0.0));
        assert_eq!((r.radius_xs, r.radius_s, r.radius_m, r.radius_xl), ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]));
        let r = radii(Sizes::new(12.0));
        assert_eq!((r.radius_xs[0], r.radius_s[0], r.radius_m[0], r.radius_l[0]), (4.0, 8.0, 12.0, 12.0));
        // (theme("desktop") depends on the session's desktop: look.rs's tests cover the colours)
        assert_eq!(theme("rounded").unwrap().cosmic().corner_radii.radius_m, [12.0; 4]);
    }
}
