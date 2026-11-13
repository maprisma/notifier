//! The desktop's look for libcosmic windows: colours and the windows' corner radius, outside
//! COSMIC too. Shared copy: the same file in every Maprisma COSMIC app; change it in all of
//! them (the rules: the maprisma-layout skill).
//!
//! - COSMIC: libcosmic's own theme (COSMIC Settings), nothing to do here.
//! - Hyprland: Maprisma Shell's colour scheme, written by Maprisma Theme
//!   ($XDG_STATE_HOME/maprisma-shell/scheme.json, Material names, hex without '#'), and
//!   Hyprland's `decoration:rounding` for the corners.
//! - KDE: Plasma's colour scheme (~/.config/kdeglobals), corners 8.
//! - anything else: libcosmic's theme as it loads.
//!
//! The same sources and keys as Maprisma Manager's `desktop.look` (desktop_look.dart), so every
//! Maprisma app shows the same colours. `MAPRISMA_DESKTOP=cosmic|kde|hyprland|other` overrides
//! the desktop (tests, screenshots). `XDG_CURRENT_DESKTOP` decides; only without it (a process
//! started outside the session's environment) the session's other marks do (desktop_in):
//! `XDG_SESSION_DESKTOP` / `DESKTOP_SESSION`, then Hyprland's instance signature (only when its
//! socket exists: a stale one stays in the systemd user environment after a Hyprland session),
//! then Plasma's `KDE_FULL_SESSION`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Cosmic,
    Kde,
    Hyprland,
    Other,
}

pub fn desktop_from(forced: Option<&str>, current: &str) -> Desktop {
    match forced.map(str::to_ascii_lowercase).as_deref() {
        Some("cosmic") => return Desktop::Cosmic,
        Some("kde") => return Desktop::Kde,
        Some("hyprland") => return Desktop::Hyprland,
        Some("other") => return Desktop::Other,
        _ => {}
    }
    let names: Vec<String> = current.split(':').map(str::to_ascii_uppercase).collect();
    if names.iter().any(|n| n == "COSMIC") {
        Desktop::Cosmic
    } else if names.iter().any(|n| n == "KDE") {
        Desktop::Kde
    } else if names.iter().any(|n| n == "HYPRLAND") {
        Desktop::Hyprland
    } else {
        Desktop::Other
    }
}

/// The session's other marks, asked only when `XDG_CURRENT_DESKTOP` is unset or empty (a
/// service or wrapper that dropped it).
#[derive(Debug, Clone, Default)]
pub struct SessionHints {
    /// `XDG_SESSION_DESKTOP` and `DESKTOP_SESSION` joined with ':' (names or session-file paths).
    pub session: String,
    /// `HYPRLAND_INSTANCE_SIGNATURE` is set and that instance's socket exists ([hyprland_socket_in]).
    pub hyprland_socket: bool,
    /// Plasma's `KDE_FULL_SESSION` is "true".
    pub kde_full_session: bool,
}

/// The desktop from the session's name(s): `KDE`, `plasma`, `plasmawayland.desktop`, `COSMIC`,
/// `hyprland-uwsm` …; Other when none is known.
pub fn session_desktop(session: &str) -> Desktop {
    for name in session.split(':') {
        let name = name.rsplit('/').next().unwrap_or("").trim().trim_end_matches(".desktop").to_ascii_lowercase();
        if name == "cosmic" || name.starts_with("cosmic-") {
            return Desktop::Cosmic;
        } else if name == "kde" || name.starts_with("plasma") {
            return Desktop::Kde;
        } else if name.starts_with("hyprland") {
            return Desktop::Hyprland;
        }
    }
    Desktop::Other
}

/// MAPRISMA_DESKTOP (when it names a desktop), else XDG_CURRENT_DESKTOP, else the session's
/// other marks.
pub fn desktop_in(forced: Option<&str>, current: &str, hints: &SessionHints) -> Desktop {
    let forced_valid = forced
        .map(str::to_ascii_lowercase)
        .is_some_and(|f| matches!(f.as_str(), "cosmic" | "kde" | "hyprland" | "other"));
    if forced_valid || !current.trim().is_empty() {
        return desktop_from(forced, current);
    }
    match session_desktop(&hints.session) {
        Desktop::Other => {}
        found => return found,
    }
    if hints.hyprland_socket {
        Desktop::Hyprland
    } else if hints.kde_full_session {
        Desktop::Kde
    } else {
        Desktop::Other
    }
}

/// Whether Hyprland instance `signature` has its socket under `runtime_dir`
/// (`$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock`). A signature that isn't a plain name is not.
pub fn hyprland_socket_in(runtime_dir: &Path, signature: &str) -> bool {
    let signature = signature.trim();
    !signature.is_empty()
        && signature.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !signature.starts_with('.')
        && runtime_dir.join("hypr").join(signature).join(".socket.sock").exists()
}

pub fn desktop() -> Desktop {
    // Unit tests never read the session's files or ask hyprctl.
    if cfg!(test) {
        return Desktop::Other;
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let forced = std::env::var("MAPRISMA_DESKTOP").ok();
    let current = var("XDG_CURRENT_DESKTOP");
    if !current.trim().is_empty() {
        // The usual case: no session marks, no file system.
        return desktop_from(forced.as_deref(), &current);
    }
    let runtime = var("XDG_RUNTIME_DIR");
    desktop_in(
        forced.as_deref(),
        &current,
        &SessionHints {
            session: format!("{}:{}", var("XDG_SESSION_DESKTOP"), var("DESKTOP_SESSION")),
            hyprland_socket: !runtime.is_empty()
                && hyprland_socket_in(Path::new(&runtime), &var("HYPRLAND_INSTANCE_SIGNATURE")),
            kde_full_session: var("KDE_FULL_SESSION").trim().eq_ignore_ascii_case("true"),
        },
    )
}

/// The desktop's colours as RGB; a missing one keeps libcosmic's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Palette {
    pub accent: Option<[u8; 3]>,
    pub background: Option<[u8; 3]>,
    pub foreground: Option<[u8; 3]>,
    pub red: Option<[u8; 3]>,
    pub yellow: Option<[u8; 3]>,
    pub green: Option<[u8; 3]>,
}

impl Palette {
    pub fn is_empty(&self) -> bool {
        self.accent.is_none() && self.background.is_none() && self.foreground.is_none()
    }

    /// Dark when the background is (dark when unknown).
    pub fn is_dark(&self) -> bool {
        self.background.map(|c| luminance(c) < 0.5).unwrap_or(true)
    }
}

fn luminance([r, g, b]: [u8; 3]) -> f32 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

fn read_small(path: &Path, max: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => home().join(fallback),
    }
}

fn hex(value: &serde_json::Value) -> Option<[u8; 3]> {
    let s = value.as_str()?.trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Maprisma Shell's scheme.json (as Maprisma Manager reads it).
pub fn shell_palette(text: &str) -> Palette {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(text) else { return Palette::default() };
    let colours = &data["colours"];
    let c = |key: &str| hex(&colours[key]);
    Palette {
        accent: c("primary"),
        background: c("surface").or_else(|| c("background")),
        foreground: c("onSurface"),
        red: c("error").or_else(|| c("red")),
        yellow: c("yellow"),
        green: c("green"),
    }
}

fn ini(text: &str, group: &str, key: &str) -> Option<String> {
    let (mut current, mut found) = (None::<&str>, None);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            current = Some(&line[1..line.len() - 1]);
        } else if current == Some(group) {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    found = Some(v.trim().to_string());
                }
            }
        }
    }
    found
}

fn kde_color(value: Option<String>) -> Option<[u8; 3]> {
    let parts: Vec<Option<u8>> = value?.split(',').map(|v| v.trim().parse::<u8>().ok()).collect();
    if (parts.len() != 3 && parts.len() != 4) || parts.iter().any(Option::is_none) {
        return None;
    }
    Some([parts[0]?, parts[1]?, parts[2]?])
}

/// Plasma's kdeglobals (as Maprisma Manager reads it).
pub fn kde_palette(text: &str) -> Palette {
    let c = |group: &str, key: &str| kde_color(ini(text, group, key));
    Palette {
        accent: c("General", "AccentColor")
            .or_else(|| c("Colors:View", "DecorationFocus"))
            .or_else(|| c("Colors:Selection", "BackgroundNormal")),
        background: c("Colors:Window", "BackgroundNormal"),
        foreground: c("Colors:Window", "ForegroundNormal"),
        red: c("Colors:View", "ForegroundNegative"),
        yellow: c("Colors:View", "ForegroundNeutral"),
        green: c("Colors:View", "ForegroundPositive"),
    }
}

/// The desktop's palette, None on COSMIC (libcosmic has it) and where nothing is known.
pub fn read_palette(desktop: Desktop) -> Option<Palette> {
    let palette = match desktop {
        Desktop::Hyprland => shell_palette(&read_small(
            &xdg("XDG_STATE_HOME", ".local/state").join("maprisma-shell/scheme.json"),
            256 * 1024,
        )?),
        Desktop::Kde => kde_palette(&read_small(&xdg("XDG_CONFIG_HOME", ".config").join("kdeglobals"), 1024 * 1024)?),
        Desktop::Cosmic | Desktop::Other => return None,
    };
    (!palette.is_empty()).then_some(palette)
}

/// The windows' corner radius where libcosmic doesn't know it: Hyprland's rounding, KDE's 8.
pub fn read_window_radius(desktop: Desktop) -> Option<f32> {
    match desktop {
        Desktop::Hyprland => {
            let out = std::process::Command::new("hyprctl")
                .args(["-j", "getoption", "decoration:rounding"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
            v["int"].as_i64().filter(|r| (0..=64).contains(r)).map(|r| r as f32)
        }
        Desktop::Kde => Some(8.0),
        Desktop::Cosmic | Desktop::Other => None,
    }
}

fn srgb([r, g, b]: [u8; 3]) -> cosmic::cosmic_theme::palette::Srgb {
    cosmic::cosmic_theme::palette::Srgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// A libcosmic theme in these colours (libcosmic derives the containers, text and states).
pub fn theme_from(palette: &Palette, radius: Option<f32>) -> cosmic::Theme {
    use cosmic::cosmic_theme::{ThemeBuilder, palette::Srgba};
    let mut builder = if palette.is_dark() { ThemeBuilder::dark() } else { ThemeBuilder::light() };
    if let Some(c) = palette.accent {
        builder = builder.accent(srgb(c));
    }
    if let Some(c) = palette.background {
        let c = srgb(c);
        builder = builder.bg_color(Srgba::new(c.red, c.green, c.blue, 1.0));
    }
    if let Some(c) = palette.foreground {
        builder = builder.text_tint(srgb(c));
    }
    if let Some(c) = palette.green {
        builder = builder.success(srgb(c));
    }
    if let Some(c) = palette.yellow {
        builder = builder.warning(srgb(c));
    }
    if let Some(c) = palette.red {
        builder = builder.destructive(srgb(c));
    }
    if let Some(r) = radius {
        builder = builder.corner_radii(crate::corners::radii(crate::corners::Sizes::new(r)));
    }
    let mut theme = cosmic::Theme::dark();
    theme.theme_type = cosmic::theme::ThemeType::Custom(Arc::new(builder.build()));
    theme
}

struct State {
    palette: Option<Palette>,
    radius: Option<f32>,
    read: std::time::Instant,
    radius_read: std::time::Instant,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

/// Reads the desktop's look again now; true when it changed.
pub fn reload() -> bool {
    let d = desktop();
    let now = std::time::Instant::now();
    let fresh = State { palette: read_palette(d), radius: read_window_radius(d), read: now, radius_read: now };
    let Ok(mut state) = STATE.write() else { return false };
    let changed = state.as_ref().is_none_or(|s| s.palette != fresh.palette || s.radius != fresh.radius);
    *state = Some(fresh);
    changed
}

/// [reload] for a periodic tick (where the app re-reads its settings): the colour files at most
/// every 5 s, Hyprland's rounding (a process) every 30 s; true when something changed.
pub fn refresh() -> bool {
    let d = desktop();
    // The read lock ends with this statement: reload() below takes the write lock (a guard held
    // through a `match` would deadlock when nothing was read yet).
    let ages = STATE
        .read()
        .ok()
        .and_then(|s| s.as_ref().map(|s| (s.read.elapsed().as_secs() >= 5, s.radius_read.elapsed().as_secs() >= 30)));
    let Some((due, radius_due)) = ages else { return reload() };
    if !due {
        return false;
    }
    let palette = read_palette(d);
    let radius = if radius_due { Some(read_window_radius(d)) } else { None };
    let Ok(mut guard) = STATE.write() else { return false };
    let Some(state) = guard.as_mut() else { return false };
    let mut changed = state.palette != palette;
    state.palette = palette;
    state.read = std::time::Instant::now();
    if let Some(radius) = radius {
        changed |= state.radius != radius;
        state.radius = radius;
        state.radius_read = state.read;
    }
    changed
}

fn with_state<T>(f: impl FnOnce(&State) -> T) -> T {
    if STATE.read().map(|s| s.is_none()).unwrap_or(true) {
        reload();
    }
    let state = STATE.read().expect("look state");
    f(state.as_ref().expect("look state loaded"))
}

/// The windows' corner radius, None where libcosmic's theme has it.
pub fn window_radius() -> Option<f32> {
    with_state(|s| s.radius)
}

/// The desktop's theme for this window, None where libcosmic's own is right (COSMIC).
pub fn theme() -> Option<cosmic::Theme> {
    with_state(|s| s.palette.as_ref().map(|p| theme_from(p, s.radius)))
}

/// `content` with a window's background in the theme's colour (for `--window` mode; an applet
/// popup's container draws its own).
pub fn window_background(theme: &cosmic::Theme) -> cosmic::iced::Background {
    cosmic::iced::Background::Color(theme.cosmic().bg_color().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops() {
        assert_eq!(desktop_from(None, "Hyprland"), Desktop::Hyprland);
        assert_eq!(desktop_from(None, "COSMIC"), Desktop::Cosmic);
        assert_eq!(desktop_from(None, "KDE"), Desktop::Kde);
        assert_eq!(desktop_from(None, "ubuntu:GNOME"), Desktop::Other);
        assert_eq!(desktop_from(Some("kde"), "Hyprland"), Desktop::Kde);
        assert_eq!(desktop_from(Some("bogus"), "COSMIC"), Desktop::Cosmic);
    }

    #[test]
    fn desktop_without_xdg_current_desktop() {
        let none = SessionHints::default();
        let hints = |s: &str, h: bool, k: bool| SessionHints { session: s.into(), hyprland_socket: h, kde_full_session: k };
        // XDG_CURRENT_DESKTOP set: the marks are not asked (a stale Hyprland socket included).
        assert_eq!(desktop_in(None, "KDE", &hints("", true, false)), Desktop::Kde);
        assert_eq!(desktop_in(None, "COSMIC", &hints("hyprland", true, false)), Desktop::Cosmic);
        assert_eq!(desktop_in(None, "ubuntu:GNOME", &hints("KDE", false, true)), Desktop::Other);
        // Unset or empty: the session's marks.
        assert_eq!(desktop_in(None, "", &none), Desktop::Other);
        assert_eq!(desktop_in(None, "", &hints("KDE:plasma", false, false)), Desktop::Kde);
        assert_eq!(desktop_in(None, " ", &hints(":/usr/share/wayland-sessions/plasmawayland.desktop", false, false)), Desktop::Kde);
        assert_eq!(desktop_in(None, "", &hints("hyprland:hyprland-uwsm", false, false)), Desktop::Hyprland);
        assert_eq!(desktop_in(None, "", &hints("COSMIC:cosmic", false, false)), Desktop::Cosmic);
        assert_eq!(desktop_in(None, "", &hints(":", true, false)), Desktop::Hyprland);
        assert_eq!(desktop_in(None, "", &hints(":", false, true)), Desktop::Kde);
        assert_eq!(desktop_in(None, "", &hints("gnome:gnome", false, false)), Desktop::Other);
        // MAPRISMA_DESKTOP still wins, "other" too; a bogus one is ignored.
        assert_eq!(desktop_in(Some("other"), "", &hints("KDE", true, true)), Desktop::Other);
        assert_eq!(desktop_in(Some("cosmic"), "", &hints("KDE", false, false)), Desktop::Cosmic);
        assert_eq!(desktop_in(Some("bogus"), "", &hints("", true, false)), Desktop::Hyprland);
        assert_eq!(desktop_in(Some("kde"), "Hyprland", &none), Desktop::Kde);
    }

    #[test]
    fn hyprland_only_with_its_socket() {
        let dir = std::env::temp_dir().join(format!("look-hypr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hypr/live_1")).unwrap();
        std::fs::write(dir.join("hypr/live_1/.socket.sock"), b"").unwrap();
        std::fs::create_dir_all(dir.join("hypr/stale_2")).unwrap();
        assert!(hyprland_socket_in(&dir, "live_1"));
        assert!(!hyprland_socket_in(&dir, "stale_2"), "signature left over, no socket");
        assert!(!hyprland_socket_in(&dir, "gone_3"));
        assert!(!hyprland_socket_in(&dir, ""));
        assert!(!hyprland_socket_in(&dir, "../hypr/live_1"));
        assert!(!hyprland_socket_in(&dir, ".."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_scheme() {
        let p = shell_palette(
            r##"{"colours":{"primary":"e78a4e","surface":"231a13","onSurface":"e8d5b7","error":"ea6962","green":"a9b665","yellow":"#d8a657"}}"##,
        );
        assert_eq!(p.accent, Some([0xe7, 0x8a, 0x4e]));
        assert_eq!(p.background, Some([0x23, 0x1a, 0x13]));
        assert_eq!(p.foreground, Some([0xe8, 0xd5, 0xb7]));
        assert_eq!(p.red, Some([0xea, 0x69, 0x62]));
        assert_eq!(p.yellow, Some([0xd8, 0xa6, 0x57]));
        assert!(p.is_dark());
        assert!(shell_palette("not json").is_empty());
        assert!(shell_palette(r#"{"colours":{"primary":"zzzzzz"}}"#).is_empty());
    }

    #[test]
    fn kde_globals() {
        let p = kde_palette(
            "[Colors:Window]\nBackgroundNormal=239,240,241\nForegroundNormal=35,38,41\n\
             [Colors:Selection]\nBackgroundNormal=61,174,233\n[Colors:View]\nForegroundNegative=218,68,83\n",
        );
        assert_eq!(p.accent, Some([61, 174, 233]));
        assert_eq!(p.background, Some([239, 240, 241]));
        assert_eq!(p.red, Some([218, 68, 83]));
        assert!(!p.is_dark());
        let p = kde_palette("[General]\nAccentColor=1,2,3\n[Colors:Selection]\nBackgroundNormal=61,174,233\n");
        assert_eq!(p.accent, Some([1, 2, 3]));
        assert_eq!(kde_palette("[Colors:Window]\nBackgroundNormal=300,0,0\n").background, None);
    }

    #[test]
    fn theme_takes_the_colours() {
        let p = Palette { accent: Some([0xe7, 0x8a, 0x4e]), background: Some([0x23, 0x1a, 0x13]), ..Default::default() };
        let t = theme_from(&p, Some(15.0));
        let bg: cosmic::iced::Color = t.cosmic().bg_color().into();
        assert!((bg.r - 0x23 as f32 / 255.0).abs() < 0.01 && (bg.b - 0x13 as f32 / 255.0).abs() < 0.01);
        assert!(t.cosmic().is_dark);
        assert_eq!(t.cosmic().corner_radii.radius_m, [15.0; 4]);
        assert_eq!(t.cosmic().corner_radii.radius_s, [8.0; 4]);
    }
}
