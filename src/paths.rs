//! Where the files live: `$XDG_CONFIG_HOME/maprisma-notifier/settings.json` and
//! `$XDG_STATE_HOME/maprisma-notifier/history.jsonl` (docs/plan.md, the contract the Manager and
//! Maprisma Shell rely on). Tests point both roots at a temporary folder with [use_roots], so they
//! never read or write the session's files.

use std::path::PathBuf;
use std::sync::OnceLock;

pub const DIR: &str = "maprisma-notifier";

static ROOTS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

/// Config and state roots for this process (tests); the first call wins.
pub fn use_roots(config: PathBuf, state: PathBuf) {
    let _ = ROOTS.set((config, state));
}

/// For tests: one temporary config/state root per test process, used by every path here.
pub fn test_roots() -> PathBuf {
    static DIR_ONCE: OnceLock<PathBuf> = OnceLock::new();
    DIR_ONCE
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("maprisma-notifier-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("config")).unwrap();
            std::fs::create_dir_all(dir.join("state")).unwrap();
            use_roots(dir.join("config"), dir.join("state"));
            dir
        })
        .clone()
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn config_root() -> PathBuf {
    if cfg!(test) {
        test_roots();
    }
    match ROOTS.get() {
        Some((config, _)) => config.clone(),
        None => xdg("XDG_CONFIG_HOME", ".config"),
    }
}

pub fn state_root() -> PathBuf {
    if cfg!(test) {
        test_roots();
    }
    match ROOTS.get() {
        Some((_, state)) => state.clone(),
        None => xdg("XDG_STATE_HOME", ".local/state"),
    }
}

pub fn settings_file() -> PathBuf {
    config_root().join(DIR).join("settings.json")
}

pub fn history_file() -> PathBuf {
    state_root().join(DIR).join("history.jsonl")
}

/// Held (flock) while the history is read for a change and written again.
pub fn history_lock() -> PathBuf {
    state_root().join(DIR).join("history.lock")
}

/// Maprisma Shell's settings (Hyprland), `notifications.dnd`.
pub fn shell_file() -> PathBuf {
    config_root().join("maprisma-shell").join("shell.json")
}

/// COSMIC's notification daemon, cosmic-config key `do_not_disturb`.
pub fn cosmic_dnd_file() -> PathBuf {
    config_root().join("cosmic/com.system76.CosmicNotifications/v1/do_not_disturb")
}

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Changes some keys of a JSON object file and keeps every other key (read–modify–write,
/// atomic). A missing or broken file starts as `{}`.
pub fn update_json(path: &std::path::Path, change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) -> std::io::Result<()> {
    let mut object = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(o) => Some(o),
            _ => None,
        })
        .unwrap_or_default();
    change(&mut object);
    let mut bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(object))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_use_a_temporary_root() {
        let root = test_roots();
        assert!(settings_file().starts_with(&root));
        assert!(history_file().starts_with(&root));
        assert!(shell_file().starts_with(&root));
        assert!(cosmic_dnd_file().starts_with(&root));
    }

    #[test]
    fn update_keeps_other_keys() {
        let path = test_roots().join("config/update-test.json");
        std::fs::write(&path, r#"{"bar": {"x": 1}, "notifications": {"position": "top"}}"#).unwrap();
        update_json(&path, |o| {
            let n = o.entry("notifications").or_insert_with(|| serde_json::json!({}));
            n["dnd"] = true.into();
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["bar"]["x"], 1);
        assert_eq!(v["notifications"]["position"], "top");
        assert_eq!(v["notifications"]["dnd"], true);
        std::fs::write(&path, "broken").unwrap();
        update_json(&path, |o| {
            o.insert("a".into(), 1.into());
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
    }
}
