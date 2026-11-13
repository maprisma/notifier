//! The AI helper every Maprisma app uses, chosen in Maprisma Manager → AI:
//! `$XDG_CONFIG_HOME/maprisma-ai/settings.json`, a JSON object whose `helper` is `"auto"` or a
//! helper id (`^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,160}$`: catalog ids like `claude`, local models
//! like `pi:<provider>/<model>`). A missing or broken file or value is `"auto"`. The Notifier only
//! reads it (when Check with AI runs and while its window shows); a chosen helper that isn't
//! installed or has no `task` falls back to auto ([crate::helpers::choose]).
//!
//! Migration: while the shared file doesn't exist, the Notifier's own old `ai_helper` setting
//! still counts.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub const DIR: &str = "maprisma-ai";
pub const AUTO: &str = "auto";
const FILE_MAX: u64 = 64 * 1024;

pub fn file() -> PathBuf {
    crate::paths::config_root().join(DIR).join("settings.json")
}

/// `"auto"` or a helper id.
pub fn valid(text: &str) -> bool {
    let mut chars = text.chars();
    text == AUTO
        || (text.len() <= 161
            && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
            && chars.all(|c| c.is_ascii_alphanumeric() || "._:/@+-".contains(c)))
}

/// The file's `helper`; "auto" when it isn't a JSON object with a valid one.
pub fn parse(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| v.get("helper").and_then(Value::as_str).filter(|h| valid(h)).map(str::to_owned))
        .unwrap_or_else(|| AUTO.into())
}

/// The choice from the file at `path`; `old` (the Notifier's own `ai_helper`) only while that
/// file doesn't exist.
pub fn choice_at(path: &Path, old: &str) -> String {
    use std::io::Read;
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if old != AUTO && crate::helpers::valid_setting(old) { old.into() } else { AUTO.into() }
        }
        _ => {
            let mut bytes = vec![];
            match std::fs::File::open(path).and_then(|f| f.take(FILE_MAX + 1).read_to_end(&mut bytes)) {
                Ok(_) if bytes.len() as u64 <= FILE_MAX => parse(&bytes),
                _ => AUTO.into(),
            }
        }
    }
}

/// The helper choice for this session (read now).
pub fn choice(old: &str) -> String {
    choice_at(&file(), old)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::paths::test_roots().join(format!("shared-ai-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn the_file_is_under_the_config_root() {
        assert_eq!(file(), crate::paths::config_root().join("maprisma-ai/settings.json"));
        assert!(file().starts_with(crate::paths::test_roots()));
    }

    #[test]
    fn valid_values() {
        for ok in ["auto", "claude", "cursor-agent", "pi:ollama/qwen3:8b", "x@y+z", &"a".repeat(161)] {
            assert!(valid(ok), "{ok}");
        }
        for bad in ["", "-x", ".x", "../x", "a b", "a\n", &"a".repeat(162), "ä"] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[test]
    fn broken_or_invalid_is_auto() {
        for text in ["", "{broken", "[]", "\"codex\"", "{}", r#"{"helper": 3}"#, r#"{"helper": "../x"}"#, r#"{"helper": ""}"#] {
            assert_eq!(parse(text.as_bytes()), "auto", "{text}");
        }
        assert_eq!(parse(br#"{"helper": "codex", "other": 1}"#), "codex");
        assert_eq!(parse(br#"{"helper": "pi:ollama/qwen"}"#), "pi:ollama/qwen");
    }

    #[test]
    fn migration_the_old_setting_counts_only_without_the_file() {
        let path = scratch("migration");
        assert_eq!(choice_at(&path, "codex"), "codex");
        assert_eq!(choice_at(&path, "auto"), "auto");
        assert_eq!(choice_at(&path, "../x"), "auto");
        // the shared file wins, even a broken one
        std::fs::write(&path, r#"{"helper": "auto"}"#).unwrap();
        assert_eq!(choice_at(&path, "codex"), "auto");
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(choice_at(&path, "codex"), "auto");
        std::fs::write(&path, r#"{"helper": "claude"}"#).unwrap();
        assert_eq!(choice_at(&path, "codex"), "claude");
        // too big: not read
        std::fs::write(&path, format!(r#"{{"helper": "claude", "pad": "{}"}}"#, "x".repeat(FILE_MAX as usize))).unwrap();
        assert_eq!(choice_at(&path, "codex"), "auto");
    }

    #[test]
    fn a_chosen_helper_that_is_not_installed_falls_back_to_auto() {
        let catalog = crate::helpers::parse(crate::helpers::SHIPPED.as_bytes()).unwrap();
        let installed: Vec<crate::helpers::Installed> =
            catalog.helpers.iter().take(2).map(|h| crate::helpers::Installed { helper: h.clone(), program: "/x".into() }).collect();
        let path = scratch("fallback");
        for (stored, used) in [("codex", "codex"), ("auto", "claude"), ("pi:ollama/qwen", "claude"), ("hermes", "claude")] {
            std::fs::write(&path, format!(r#"{{"helper": "{stored}"}}"#)).unwrap();
            let choice = choice_at(&path, "auto");
            assert_eq!(crate::helpers::choose(&choice, &installed).unwrap().helper.id, used, "{stored}");
        }
        assert_eq!(crate::helpers::choose(&choice_at(&path, "auto"), &[]), None);
    }
}
