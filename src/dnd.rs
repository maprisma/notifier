//! Do not disturb on each desktop (docs/plan.md): the recorder applies the effective state
//! (`dnd || schedule`) where the desktop's own notification popups read it.
//!
//! | Desktop  | Where |
//! |----------|-------|
//! | Hyprland | `~/.config/maprisma-shell/shell.json` `"notifications": {"dnd": bool}` (Shell watches it) |
//! | COSMIC   | cosmic-config `com.system76.CosmicNotifications` v1 `do_not_disturb` (`true`/`false`) |
//! | KDE      | `org.freedesktop.Notifications.Inhibit`, held while in effect ([Kde]) |

use crate::look::Desktop;
use serde_json::Value;

/// Hyprland: `notifications.dnd` in Maprisma Shell's settings, the other keys kept. Written only
/// when it differs. Returns whether the file was written.
pub fn set_shell(on: bool) -> std::io::Result<bool> {
    let path = crate::paths::shell_file();
    let current = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("notifications").and_then(|n| n.get("dnd")).and_then(Value::as_bool));
    if current == Some(on) {
        return Ok(false);
    }
    crate::paths::update_json(&path, |object| {
        let notifications = object.entry("notifications").or_insert_with(|| Value::Object(Default::default()));
        if !notifications.is_object() {
            *notifications = Value::Object(Default::default());
        }
        notifications["dnd"] = on.into();
    })?;
    Ok(true)
}

/// COSMIC: the cosmic-config key file (RON `true` / `false`), written only when it differs.
pub fn set_cosmic(on: bool) -> std::io::Result<bool> {
    let path = crate::paths::cosmic_dnd_file();
    let current = std::fs::read_to_string(&path).ok().map(|t| t.trim() == "true");
    if current == Some(on) {
        return Ok(false);
    }
    crate::paths::write_atomic(&path, if on { b"true" } else { b"false" })?;
    Ok(true)
}

/// KDE: Plasma's notification server holds back popups while an inhibition is held; it ends
/// with `UnInhibit` or when the recorder's connection closes.
#[derive(Default)]
pub struct Kde {
    cookie: Option<u32>,
}

impl Kde {
    pub fn held(&self) -> bool {
        self.cookie.is_some()
    }

    pub async fn set(&mut self, connection: &zbus::Connection, on: bool) -> zbus::Result<()> {
        let proxy = zbus::Proxy::new(connection, crate::notify::INTERFACE, crate::notify::PATH, crate::notify::INTERFACE).await?;
        match (on, self.cookie) {
            (true, None) => {
                let hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> = Default::default();
                let cookie: u32 = proxy.call("Inhibit", &("io.github.ubruckhaus.CosmicMaprismaNotifier", crate::i18n::tr("Do not disturb"), hints)).await?;
                self.cookie = Some(cookie);
            }
            (false, Some(cookie)) => {
                self.cookie = None;
                proxy.call::<_, _, ()>("UnInhibit", &(cookie,)).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Where the effective state goes on this desktop (None: nowhere, e.g. another desktop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Shell,
    Cosmic,
    Kde,
}

pub fn target(desktop: Desktop) -> Option<Target> {
    match desktop {
        Desktop::Hyprland => Some(Target::Shell),
        Desktop::Cosmic => Some(Target::Cosmic),
        Desktop::Kde => Some(Target::Kde),
        Desktop::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_per_desktop() {
        use crate::look::desktop_from;
        assert_eq!(target(desktop_from(None, "Hyprland")), Some(Target::Shell));
        assert_eq!(target(desktop_from(None, "COSMIC")), Some(Target::Cosmic));
        assert_eq!(target(desktop_from(None, "KDE")), Some(Target::Kde));
        assert_eq!(target(desktop_from(None, "GNOME")), None);
    }

    #[test]
    fn shell_and_cosmic_files() {
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let shell = crate::paths::shell_file();
        std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
        std::fs::write(&shell, r#"{"bar": {"left": ["a"]}, "notifications": {"position": "top-left", "timeout": 9}}"#).unwrap();
        assert!(set_shell(true).unwrap());
        assert!(!set_shell(true).unwrap());
        let v: Value = serde_json::from_slice(&std::fs::read(&shell).unwrap()).unwrap();
        assert_eq!(v["notifications"], serde_json::json!({"position": "top-left", "timeout": 9, "dnd": true}));
        assert_eq!(v["bar"]["left"][0], "a");
        assert!(set_shell(false).unwrap());
        std::fs::write(&shell, r#"{"notifications": 3}"#).unwrap();
        assert!(set_shell(true).unwrap());
        let v: Value = serde_json::from_slice(&std::fs::read(&shell).unwrap()).unwrap();
        assert_eq!(v["notifications"]["dnd"], true);

        let cosmic = crate::paths::cosmic_dnd_file();
        let _ = std::fs::remove_file(&cosmic);
        assert!(set_cosmic(false).unwrap());
        assert_eq!(std::fs::read_to_string(&cosmic).unwrap(), "false");
        assert!(set_cosmic(true).unwrap());
        assert!(!set_cosmic(true).unwrap());
        assert_eq!(std::fs::read_to_string(&cosmic).unwrap(), "true");
    }
}
