//! Layout rule 5a: the size the user gave the `--window` window, remembered for the next open.
//! The same file in every Maprisma Rust app: the app keeps `to_value(size)` under `KEY` in its own
//! settings file and starts the window with `parse(saved).unwrap_or(default)`. Nothing here
//! touches files, so all of it is tested below. On Hyprland the window's place is remembered too
//! (`POSITION_KEY`): read from hyprctl when it closes, and put back with a move after it opens
//! (a Wayland window can't place itself; Hyprland's tray rule puts it under the bar first).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The settings key.
pub const KEY: &str = "window_size";
/// Smaller than this the window is of no use; larger than `MAX` no screen is.
pub const MIN: (f32, f32) = (320.0, 240.0);
pub const MAX: f32 = 4096.0;
/// Saved this long after the last resize event (a drag sends many).
pub const SETTLE: Duration = Duration::from_millis(500);

pub type Size = (f32, f32);

/// A usable size (whole logical pixels, within MIN..MAX), or None for nonsense.
pub fn clamp(width: f32, height: f32) -> Option<Size> {
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then(|| (width.clamp(MIN.0, MAX).round(), height.clamp(MIN.1, MAX).round()))
}

/// `{"width": w, "height": h}` from the settings; a missing or broken value is None.
pub fn parse(value: Option<&Value>) -> Option<Size> {
    let value = value?;
    clamp(value.get("width")?.as_f64()? as f32, value.get("height")?.as_f64()? as f32)
}

pub fn to_value(size: Size) -> Value {
    json!({ "width": size.0 as u32, "height": size.1 as u32 })
}

/// Collects resize events and says when (and what) to save.
#[derive(Debug, Default)]
pub struct Debounce {
    pending: Option<(Size, Instant)>,
    saved: Option<Size>,
}

impl Debounce {
    /// `start`: the size the window was opened with (not written again).
    pub fn new(start: Size) -> Self {
        Self { pending: None, saved: clamp(start.0, start.1) }
    }

    pub fn resized(&mut self, width: f32, height: f32, now: Instant) {
        if let Some(size) = clamp(width, height) {
            self.pending = Some((size, now));
        }
    }

    /// A resize is waiting to be saved (keep the timer running).
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// The size to save now: `SETTLE` after the last resize, and only when it changed.
    /// `ignore` (asked only when something is to be saved): the window is maximised, fullscreen
    /// or tiled; that size is not the user's. See [not_users_size].
    pub fn due(&mut self, now: Instant, ignore: impl FnOnce() -> bool) -> Option<Size> {
        let (_, at) = self.pending?;
        if now.duration_since(at) < SETTLE {
            return None;
        }
        self.flush(ignore)
    }

    /// The window closes: whatever is waiting, at once.
    pub fn flush(&mut self, ignore: impl FnOnce() -> bool) -> Option<Size> {
        let (size, _) = self.pending.take()?;
        if self.saved == Some(size) || ignore() {
            return None;
        }
        self.saved = Some(size);
        Some(size)
    }
}

/// Whether the window's present size is not the user's own choice. libcosmic's sharp corners
/// (`core.window.sharp_corners`: maximised, fullscreen or tiled) say so on COSMIC and KDE; Hyprland
/// marks every window as tiled, floating ones too, so there hyprctl says whether this process's
/// window floats and isn't fullscreen. Spawns hyprctl: call it only when a size is to be saved.
pub fn not_users_size(sharp_corners: bool) -> bool {
    // In a Hyprland session (never in unit tests: they must not ask the real one).
    if cfg!(test) || !on_hyprland() {
        return sharp_corners;
    }
    let out = std::process::Command::new("hyprctl")
        .args(["-j", "clients"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(out) => hypr_not_users(&out.stdout, std::process::id()).unwrap_or(sharp_corners),
        Err(_) => sharp_corners,
    }
}

/// Hyprland as look.rs decides the desktop: `MAPRISMA_DESKTOP`, else `XDG_CURRENT_DESKTOP`; only
/// without both, Hyprland's instance signature with its socket present. A stale
/// `HYPRLAND_INSTANCE_SIGNATURE` stays in the systemd user environment after a Hyprland session,
/// so it never counts in a KDE or COSMIC session.
pub fn hyprland_session(forced: &str, current: &str, socket: impl FnOnce() -> bool) -> bool {
    let forced = forced.trim().to_ascii_lowercase();
    if matches!(forced.as_str(), "cosmic" | "kde" | "hyprland" | "other") {
        return forced == "hyprland";
    }
    if !current.trim().is_empty() {
        return current.split(':').any(|d| d.trim().eq_ignore_ascii_case("hyprland"));
    }
    socket()
}

fn on_hyprland() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    hyprland_session(&var("MAPRISMA_DESKTOP"), &var("XDG_CURRENT_DESKTOP"), || {
        let (runtime, signature) = (var("XDG_RUNTIME_DIR"), var("HYPRLAND_INSTANCE_SIGNATURE"));
        let signature = signature.trim();
        !runtime.is_empty()
            && !signature.is_empty()
            && signature.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && !signature.starts_with('.')
            && std::path::Path::new(&runtime).join("hypr").join(signature).join(".socket.sock").exists()
    })
}

/// From `hyprctl -j clients`: Some(true) when this pid's window is tiled or fullscreen,
/// Some(false) when it floats; None when the window isn't listed.
pub fn hypr_not_users(clients: &[u8], pid: u32) -> Option<bool> {
    let list: Value = serde_json::from_slice(clients).ok()?;
    let me = list.as_array()?.iter().find(|c| c["pid"].as_u64() == Some(pid as u64))?;
    let floating = me["floating"].as_bool()?;
    let fullscreen = me["fullscreen"].as_u64().map(|f| f != 0).or_else(|| me["fullscreen"].as_bool()).unwrap_or(false);
    Some(!floating || fullscreen)
}

/// The settings key of the window's place on Hyprland.
pub const POSITION_KEY: &str = "window_position";

pub type Position = (i32, i32);

/// `{"x": x, "y": y}` from the settings; a missing or broken value is None.
pub fn parse_position(value: Option<&Value>) -> Option<Position> {
    let value = value?;
    let get = |k: &str| value.get(k)?.as_i64().and_then(|v| i32::try_from(v).ok());
    Some((get("x")?, get("y")?))
}

pub fn position_value(position: Position) -> Value {
    json!({ "x": position.0, "y": position.1 })
}

/// From `hyprctl -j clients`: where this pid's window is, when it floats and isn't fullscreen
/// (a tiled or fullscreen place is not the user's).
pub fn hypr_position(clients: &[u8], pid: u32) -> Option<Position> {
    if hypr_not_users(clients, pid)? {
        return None;
    }
    let list: Value = serde_json::from_slice(clients).ok()?;
    let me = list.as_array()?.iter().find(|c| c["pid"].as_u64() == Some(pid as u64))?;
    let at = me["at"].as_array()?;
    Some((i32::try_from(at.first()?.as_i64()?).ok()?, i32::try_from(at.get(1)?.as_i64()?).ok()?))
}

/// From `hyprctl -j monitors`: whether a window placed at `position` (its top left corner) with
/// `size` stays at least 48 px on a screen, so a place on a screen that is gone isn't used.
pub fn on_a_monitor(monitors: &[u8], position: Position, size: Size) -> bool {
    let Ok(list) = serde_json::from_slice::<Value>(monitors) else { return false };
    let Some(list) = list.as_array() else { return false };
    let (x, y) = (position.0 as f64, position.1 as f64);
    let w = size.0 as f64;
    list.iter().any(|m| {
        let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
        let (Some(mx), Some(my), Some(mw), Some(mh)) = (m["x"].as_f64(), m["y"].as_f64(), m["width"].as_f64(), m["height"].as_f64()) else {
            return false;
        };
        let (mw, mh) = (mw / scale, mh / scale);
        x + w - 48.0 >= mx && x + 48.0 <= mx + mw && y >= my && y + 48.0 <= my + mh
    })
}

/// An off-screen screenshot run (`MAPRISMA_OFFSHOT`, set by maprisma-layout's offshot.sh): the
/// window stays where the run put it and its place there is never saved over the user's.
fn offshot() -> bool {
    std::env::var_os("MAPRISMA_OFFSHOT").is_some_and(|v| !v.is_empty())
}

fn hyprctl(args: &[&str]) -> Option<Vec<u8>> {
    let out = std::process::Command::new("hyprctl")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Where the user left this process's window (Hyprland only; None elsewhere, in unit tests, or
/// when it is tiled or fullscreen). Spawns hyprctl: call it when the window closes.
pub fn current_position() -> Option<Position> {
    if cfg!(test) || offshot() || !on_hyprland() {
        return None;
    }
    hypr_position(&hyprctl(&["-j", "clients"])?, std::process::id())
}

/// Puts this process's window back at `position` once Hyprland lists it (asked every 50 ms for
/// up to 3 s, in a thread of its own), when the place is still on a screen. Hyprland only.
pub fn restore_position(position: Option<Position>, size: Size) {
    let Some(position) = position else { return };
    if cfg!(test) || offshot() || !on_hyprland() {
        return;
    }
    let pid = std::process::id();
    std::thread::spawn(move || {
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(50));
            let Some(clients) = hyprctl(&["-j", "clients"]) else { return };
            if hypr_not_users(&clients, pid).is_none() {
                continue; // not mapped yet
            }
            if hypr_position(&clients, pid) == Some(position) {
                return;
            }
            if hyprctl(&["-j", "monitors"]).is_some_and(|m| on_a_monitor(&m, position, size)) {
                let lua = format!("hl.dispatch(hl.dsp.window.move({{ x = {}, y = {}, window = 'pid:{pid}' }}))", position.0, position.1);
                hyprctl(&["eval", &lua]);
            }
            return;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_and_parse() {
        assert_eq!(clamp(500.4, 700.6), Some((500.0, 701.0)));
        assert_eq!(clamp(10.0, 10.0), Some(MIN));
        assert_eq!(clamp(9000.0, 9000.0), Some((MAX, MAX)));
        assert_eq!(clamp(0.0, 500.0), None);
        assert_eq!(clamp(f32::NAN, 500.0), None);
        assert_eq!(clamp(f32::INFINITY, 500.0), None);
        assert_eq!(parse(Some(&json!({ "width": 600, "height": 800 }))), Some((600.0, 800.0)));
        assert_eq!(parse(Some(&json!({ "width": 600.0, "height": 100 }))), Some((600.0, MIN.1)));
        assert_eq!(parse(Some(&json!({ "width": "600", "height": 800 }))), None);
        assert_eq!(parse(Some(&json!({ "width": 600 }))), None);
        assert_eq!(parse(Some(&json!([600, 800]))), None);
        assert_eq!(parse(Some(&json!({ "width": -5, "height": 800 }))), None);
        assert_eq!(parse(None), None);
        assert_eq!(parse(Some(&to_value((640.0, 480.0)))), Some((640.0, 480.0)));
    }

    #[test]
    fn saves_once_the_resizing_settles() {
        let start = Instant::now();
        let mut d = Debounce::new((400.0, 860.0));
        // Opening at the start size writes nothing.
        d.resized(400.0, 860.0, start);
        assert_eq!(d.due(start + SETTLE, || false), None);
        assert!(!d.waiting());
        // A drag: only the last size, only after SETTLE.
        d.resized(450.0, 860.0, start);
        d.resized(500.0, 900.0, start + Duration::from_millis(100));
        assert!(d.waiting());
        assert_eq!(d.due(start + Duration::from_millis(400), || false), None);
        assert_eq!(d.due(start + Duration::from_millis(600), || false), Some((500.0, 900.0)));
        assert!(!d.waiting());
        // Maximised / tiled sizes are dropped.
        d.resized(1920.0, 1080.0, start);
        assert_eq!(d.due(start + SETTLE, || true), None);
        assert!(!d.waiting());
        // Closing right after a resize saves it at once.
        d.resized(520.0, 910.0, start);
        assert_eq!(d.flush(|| false), Some((520.0, 910.0)));
        assert_eq!(d.flush(|| false), None);
        // Nothing to save: `ignore` is not asked (no hyprctl per tick).
        assert_eq!(d.due(start + SETTLE, || panic!("asked")), None);
    }

    #[test]
    fn hyprland_only_in_a_hyprland_session() {
        // XDG_CURRENT_DESKTOP decides: a stale signature/socket in KDE or COSMIC is not asked.
        assert!(!hyprland_session("", "KDE", || panic!("asked")));
        assert!(!hyprland_session("", "COSMIC", || panic!("asked")));
        assert!(hyprland_session("", "Hyprland", || panic!("asked")));
        assert!(hyprland_session("", "foo:hyprland", || false));
        // MAPRISMA_DESKTOP first; a bogus one is ignored.
        assert!(!hyprland_session("kde", "Hyprland", || true));
        assert!(hyprland_session("hyprland", "KDE", || false));
        assert!(!hyprland_session("bogus", "KDE", || true));
        // Neither set: only a live socket.
        assert!(hyprland_session("", "", || true));
        assert!(!hyprland_session("", " ", || false));
    }

    #[test]
    fn hyprland_floating_is_the_users() {
        let clients = br#"[{"pid": 7, "floating": false, "fullscreen": 0},
                           {"pid": 42, "floating": true, "fullscreen": 0},
                           {"pid": 43, "floating": true, "fullscreen": 2}]"#;
        assert_eq!(hypr_not_users(clients, 42), Some(false));
        assert_eq!(hypr_not_users(clients, 7), Some(true));
        assert_eq!(hypr_not_users(clients, 43), Some(true));
        assert_eq!(hypr_not_users(clients, 99), None);
        assert_eq!(hypr_not_users(b"oops", 42), None);
    }

    #[test]
    fn positions_from_hyprctl_and_settings() {
        let clients = br#"[{"pid": 7, "floating": true, "fullscreen": 0, "at": [2860, 42], "size": [560, 700]},
                           {"pid": 8, "floating": false, "fullscreen": 0, "at": [0, 0], "size": [10, 10]}]"#;
        assert_eq!(hypr_position(clients, 7), Some((2860, 42)));
        assert_eq!(hypr_position(clients, 8), None); // tiled
        assert_eq!(hypr_position(clients, 9), None);
        assert_eq!(parse_position(Some(&position_value((-20, 42)))), Some((-20, 42)));
        assert_eq!(parse_position(Some(&json!({"x": "1"}))), None);
        assert_eq!(parse_position(None), None);
    }

    #[test]
    fn a_place_must_stay_on_a_screen() {
        // 4K at scale 2: 1920×1080 logical
        let monitors = br#"[{"x": 0, "y": 0, "width": 3840, "height": 2160, "scale": 2.0}]"#;
        assert!(on_a_monitor(monitors, (1300, 42), (560.0, 700.0)));
        assert!(on_a_monitor(monitors, (-500, 42), (560.0, 700.0))); // 60 px still on it
        assert!(!on_a_monitor(monitors, (2000, 42), (560.0, 700.0)));
        assert!(!on_a_monitor(monitors, (100, 1070), (560.0, 700.0)));
        assert!(!on_a_monitor(b"nonsense", (0, 0), (560.0, 700.0)));
    }
}
