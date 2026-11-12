//! Layout rule 5a: the size the user gave the `--window` window, remembered for the next open.
//! The same file in every Maprisma Rust app: the app keeps `to_value(size)` under `KEY` in its own
//! settings file and starts the window with `parse(saved).unwrap_or(default)`. Nothing here
//! touches files, so all of it is tested below.

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
    if cfg!(test) || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none_or(|v| v.is_empty()) {
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

/// From `hyprctl -j clients`: Some(true) when this pid's window is tiled or fullscreen,
/// Some(false) when it floats; None when the window isn't listed.
pub fn hypr_not_users(clients: &[u8], pid: u32) -> Option<bool> {
    let list: Value = serde_json::from_slice(clients).ok()?;
    let me = list.as_array()?.iter().find(|c| c["pid"].as_u64() == Some(pid as u64))?;
    let floating = me["floating"].as_bool()?;
    let fullscreen = me["fullscreen"].as_u64().map(|f| f != 0).or_else(|| me["fullscreen"].as_bool()).unwrap_or(false);
    Some(!floating || fullscreen)
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
}
