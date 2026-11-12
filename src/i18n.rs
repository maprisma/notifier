//! Bundled catalogs (translations/*.json, compiled by tools/gen-translations) shared by the
//! applet, the window and the tray. The setting is `language` in settings.json: "system" or a code.
use serde_json::Value;
use std::{path::PathBuf, sync::{LazyLock, Mutex}, time::{Duration, Instant}};
pub const LANGUAGES: [(&str, &str); 8] = [("en", "English"), ("de", "Deutsch"), ("fr", "Français"), ("es", "Español"), ("it", "Italiano"), ("pt", "Português"), ("nl", "Nederlands"), ("pl", "Polski")];
include!("translations_generated.rs");
static CATALOGS: LazyLock<Vec<Value>> = LazyLock::new(|| CATALOG_JSON.iter().map(|s| serde_json::from_str(s).unwrap()).collect());
pub fn system_language(values: [&str; 3]) -> &'static str {
    let value = values.into_iter().find(|v| !v.is_empty()).unwrap_or("en");
    let code = value.split(['_', '.', '@', '-']).next().unwrap_or("").to_lowercase();
    LANGUAGES.iter().find(|(c, _)| *c == code).map_or("en", |(c, _)| *c)
}
fn system() -> &'static str {
    let values = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|k| std::env::var(k).unwrap_or_default());
    system_language([&values[0], &values[1], &values[2]])
}
fn config_path() -> PathBuf {
    crate::paths::settings_file()
}
fn read_file_setting() -> String {
    let value = std::fs::read(config_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["language"].as_str().map(str::to_owned)).unwrap_or_default();
    if LANGUAGES.iter().any(|(c, _)| *c == value) { value } else { "system".into() }
}
// Re-read at most once a second (a change in another interface takes effect soon; a view
// with many texts doesn't read the file for each).
static CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
fn read_setting() -> String {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match &*cache {
        Some((at, value)) if at.elapsed() < Duration::from_secs(1) => value.clone(),
        _ => {
            let value = read_file_setting();
            *cache = Some((Instant::now(), value.clone()));
            value
        }
    }
}
fn forget() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}
pub fn set_language(code: &str) -> std::io::Result<()> {
    let code = if code == "auto" { "system" } else { code };
    if code != "system" && !LANGUAGES.iter().any(|(c, _)| *c == code) { return Ok(()); }
    let result = write_pref("language", Some(code.into()));
    forget();
    result
}
/// One interface setting from config.json (language, screenshot_key, corners).
pub fn read_pref(key: &str) -> Option<Value> {
    std::fs::read(config_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get(key).cloned())
}
/// Sets (Some) or removes (None) one setting; the other keys stay as they are.
pub fn write_pref(key: &str, value: Option<Value>) -> std::io::Result<()> {
    crate::settings::set(key, value)
}
pub fn tr(key: &str) -> String {
    // Re-read to make changes in another running interface take effect too.
    let setting = read_setting();
    let code = if setting == "system" { system() } else { &setting };
    let index = LANGUAGES.iter().position(|(c, _)| *c == code).unwrap_or(0);
    CATALOGS[index][key].as_str().unwrap_or(key).to_owned()
}
pub fn trf(key: &str, values: &[String]) -> String {
    let text = tr(key);
    let mut result = String::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find('}') {
            if let Ok(index) = rest[1..end].parse::<usize>() {
                if let Some(value) = values.get(index) { result.push_str(value); rest = &rest[end+1..]; continue; }
            }
        }
        result.push('{'); rest = &rest[1..];
    }
    result.push_str(rest); result
}
pub fn language_options() -> Vec<(String, String)> {
    let code = system();
    let name = LANGUAGES.iter().find(|(c, _)| *c == code).unwrap().1;
    let selected = read_setting();
    std::iter::once(("system", trf("System ({0})", &[name.into()])))
        .chain(LANGUAGES.iter().map(|(c, n)| (*c, n.to_string())))
        .map(|(c, label)| (c.into(), if selected == c { format!("✓ {label}") } else { label })).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn locale_precedence() {
        for (input, expected) in [(["de_DE.UTF-8", "fr", "pl"], "de"), (["", "fr-FR", "de"], "fr"), (["", "", "pl_PL@euro"], "pl"), (["zz", "de", "fr"], "en"), (["C", "de", "fr"], "en"), (["POSIX", "", "de"], "en"), (["", "", ""], "en")] { assert_eq!(system_language(input), expected); }
    }
    fn placeholders(text: &str) -> Vec<String> {
        let mut found: Vec<String> = text.match_indices('{').filter_map(|(i, _)| text[i..].find('}').map(|e| text[i..i + e + 1].to_owned())).filter(|p| p[1..p.len() - 1].parse::<usize>().is_ok()).collect();
        found.sort();
        found
    }
    /// Every tr()/trf() text in the sources is in all eight catalogs, with the same placeholders.
    #[test] fn every_text_in_every_language() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys = std::collections::BTreeSet::new();
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() { dirs.push(path); continue; }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("translations_generated.rs") { continue; }
                let source = std::fs::read_to_string(&path).unwrap();
                for start in ["tr(\"", "trf(\""] {
                    for (i, _) in source.match_indices(start) {
                        if source[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_') { continue; }
                        let rest = &source[i + start.len() - 1..];
                        let mut de = serde_json::Deserializer::from_str(rest).into_iter::<String>();
                        if let Some(Ok(key)) = de.next() { keys.insert(key); }
                    }
                }
            }
        }
        assert!(keys.len() > 30, "{}", keys.len());
        for (index, (code, _)) in LANGUAGES.iter().enumerate() {
            for key in &keys {
                let text = CATALOGS[index][key.as_str()].as_str().unwrap_or_else(|| panic!("{code}: {key}"));
                assert_eq!(placeholders(key), placeholders(text), "{code}: {key}");
            }
        }
    }
    /// src/translations_generated.rs is what tools/gen-translations makes of translations/.
    #[test] fn generated_catalogs_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (index, (code, _)) in LANGUAGES.iter().enumerate() {
            let file: Value = serde_json::from_slice(&std::fs::read(root.join(format!("translations/{code}.json"))).unwrap()).unwrap();
            assert_eq!(file, CATALOGS[index], "{code}: run tools/gen-translations");
        }
    }
}
