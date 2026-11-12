//! AI helpers for "Check with AI" (docs/plan.md "Crashes"): a catalog shared with Maprisma
//! (`helpers.json`, made from Maprisma's agent adapters), every helper with a `task` command
//! (an interactive session started with a first prompt) whose program is installed.
//!
//! The catalog: Maprisma's `/usr/lib/maprisma/data/helpers.json` when it is there, parses and
//! is at least as new (`version`) as the copy shipped here (`data/helpers.json`, built in and
//! installed to `~/.local/share/maprisma-notifier/helpers.json` for Maprisma Manager).

use serde_json::Value;
use std::path::{Path, PathBuf};

pub const SYSTEM_CATALOG: &str = "/usr/lib/maprisma/data/helpers.json";
pub const SHIPPED: &str = include_str!("../data/helpers.json");
pub const PROMPT: &str = "{prompt}";
const CATALOG_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Helper {
    pub id: String,
    pub name: String,
    pub command: String,
    /// The interactive session: argv with a `{prompt}` element.
    pub task: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub version: i64,
    /// In preference order; only helpers with a usable `task`.
    pub helpers: Vec<Helper>,
}

/// An id or a command: a plain name (no path, no spaces, not an option).
pub fn plain(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && !text.starts_with(['-', '.']) && text.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

fn helper(value: &Value) -> Option<Helper> {
    let id = value.get("id")?.as_str().filter(|s| plain(s))?.to_owned();
    let command = value.get("command")?.as_str().filter(|s| plain(s))?.to_owned();
    let name = value.get("name").and_then(Value::as_str).map(|n| crate::crash::clean(n, 64)).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
    let argv = value.get("task")?.get("argv")?.as_array()?;
    let task: Vec<String> = argv.iter().map(|a| a.as_str().map(str::to_owned)).collect::<Option<_>>()?;
    if task.is_empty() || task.len() > 64 || task.iter().all(|a| !a.contains(PROMPT)) || task[0].contains(PROMPT) || task.iter().any(|a| a.contains('\0')) {
        return None;
    }
    Some(Helper { id, name, command, task })
}

/// A catalog; None when it isn't one. Helpers without a task, or with broken fields, are left
/// out; a repeated id counts once (the first).
pub fn parse(bytes: &[u8]) -> Option<Catalog> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let version = value.get("version")?.as_i64()?;
    let mut helpers: Vec<Helper> = vec![];
    for found in value.get("helpers")?.as_array()?.iter().filter_map(helper) {
        if !helpers.iter().any(|h| h.id == found.id) {
            helpers.push(found);
        }
    }
    Some(Catalog { version, helpers })
}

/// Maprisma's catalog when it is at least as new as the shipped one.
pub fn pick(system: Option<Catalog>, shipped: Catalog) -> Catalog {
    match system {
        Some(system) if system.version >= shipped.version => system,
        _ => shipped,
    }
}

fn read_catalog(path: &Path) -> Option<Catalog> {
    use std::io::Read;
    let mut bytes = vec![];
    std::fs::File::open(path).ok()?.take(CATALOG_MAX).read_to_end(&mut bytes).ok()?;
    parse(&bytes)
}

pub fn catalog() -> Catalog {
    let shipped = parse(SHIPPED.as_bytes()).expect("data/helpers.json");
    let system = if cfg!(test) { None } else { read_catalog(Path::new(SYSTEM_CATALOG)) };
    pick(system, shipped)
}

/// A program: on `path`, in `~/.local/bin` or `~/.npm-global/bin`, else where mise has it
/// (`mise which`). Nothing that would install it is ever run.
pub fn find_command(command: &str, path: &str, home: &Path, mise: Option<&Path>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let extra = [home.join(".local/bin"), home.join(".npm-global/bin")];
    let extra: Vec<&str> = extra.iter().filter_map(|p| p.to_str()).collect();
    let search = std::iter::once(path).chain(extra).collect::<Vec<_>>().join(":");
    if let Some(found) = crate::crash::which_in(command, &search) {
        return Some(found);
    }
    let mise = mise?.to_str()?;
    let out = PathBuf::from(crate::crash::first_line(&[mise, "which", command], std::time::Duration::from_secs(5))?);
    (out.is_absolute() && std::fs::metadata(&out).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)).then_some(out)
}

/// A helper whose program was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub helper: Helper,
    pub program: PathBuf,
}

/// The catalog's helpers that are installed, in its order.
pub fn installed_in(catalog: &Catalog, path: &str, home: &Path, mise: Option<&Path>) -> Vec<Installed> {
    catalog
        .helpers
        .iter()
        .filter_map(|h| find_command(&h.command, path, home, mise).map(|program| Installed { helper: h.clone(), program }))
        .collect()
}

/// The installed helpers on this session (its PATH, the user's folders, mise).
pub fn installed() -> Vec<Installed> {
    let path = crate::crash::env_path();
    let home = crate::paths::home();
    let mise = find_command("mise", &path, &home, None);
    installed_in(&catalog(), &path, &home, mise.as_deref())
}

/// The `ai_helper` setting: "auto" or an id.
pub fn valid_setting(text: &str) -> bool {
    text == "auto" || plain(text)
}

/// The helper to use: the chosen one when installed, else (auto, unknown, not installed) the first.
pub fn choose<'a>(setting: &str, installed: &'a [Installed]) -> Option<&'a Installed> {
    installed.iter().find(|i| i.helper.id == setting).or_else(|| installed.first())
}

/// The helper's task argv: its program where the catalog names the command, `{prompt}` replaced
/// inside single elements (never through a shell). A prompt alone in an element never starts
/// with `-`, so it can't be read as an option.
pub fn task_argv(installed: &Installed, prompt: &str) -> Vec<String> {
    let prompt: String = prompt.chars().filter(|c| *c != '\0').collect();
    let lone = if prompt.starts_with('-') { format!(" {prompt}") } else { prompt.clone() };
    installed
        .helper
        .task
        .iter()
        .enumerate()
        .map(|(i, arg)| {
            if i == 0 && *arg == installed.helper.command {
                installed.program.to_string_lossy().into_owned()
            } else if arg == PROMPT {
                lone.clone()
            } else {
                arg.replace(PROMPT, &prompt)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash::tests::{program, scratch};

    #[test]
    fn shipped_catalog() {
        let c = parse(SHIPPED.as_bytes()).unwrap();
        assert!(c.version >= 1);
        let ids: Vec<&str> = c.helpers.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(&ids[..2], ["claude", "codex"]);
        assert_eq!(c.helpers[0].task, vec!["claude", "{prompt}"]);
        assert_eq!(catalog(), c);
    }

    #[test]
    fn catalog_parse_skips_bad_entries() {
        let text = br#"{"version": 3, "helpers": [
            {"id": "good", "name": "Good", "command": "good", "task": {"argv": ["good", "--x", "{prompt}"]}, "verified": true},
            {"id": "noask", "command": "noask", "ask": {"argv": ["noask", "{prompt}"]}, "task": null},
            {"id": "nocmd", "name": "x", "task": {"argv": ["x", "{prompt}"]}},
            {"id": "../evil", "command": "evil", "task": {"argv": ["evil", "{prompt}"]}},
            {"id": "path", "command": "/bin/sh", "task": {"argv": ["/bin/sh", "{prompt}"]}},
            {"id": "noprompt", "command": "np", "task": {"argv": ["np"]}},
            {"id": "first", "command": "f", "task": {"argv": ["{prompt}"]}},
            {"id": "nums", "command": "n", "task": {"argv": ["n", 3, "{prompt}"]}},
            {"id": "empty", "command": "e", "task": {"argv": []}},
            "garbage",
            {"id": "good", "command": "again", "task": {"argv": ["again", "{prompt}"]}},
            {"id": "noname", "command": "nn", "task": {"argv": ["nn", "--p={prompt}"]}}
        ]}"#;
        let c = parse(text).unwrap();
        assert_eq!(c.version, 3);
        let ids: Vec<&str> = c.helpers.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["good", "noname"]);
        assert_eq!(c.helpers[1].name, "noname");
        for bad in [&b""[..], b"[]", b"{}", br#"{"version": "1", "helpers": []}"#, br#"{"version": 1}"#, br#"{"version": 1, "helpers": {}}"#] {
            assert_eq!(parse(bad), None);
        }
        assert_eq!(parse(br#"{"version": 1, "helpers": []}"#).unwrap().helpers, vec![]);
    }

    #[test]
    fn newer_system_catalog_wins() {
        let shipped = parse(br#"{"version": 2, "helpers": []}"#).unwrap();
        let older = parse(br#"{"version": 1, "helpers": []}"#).unwrap();
        let same = parse(br#"{"version": 2, "helpers": [{"id": "a", "command": "a", "task": {"argv": ["a", "{prompt}"]}}]}"#).unwrap();
        assert_eq!(pick(None, shipped.clone()), shipped);
        assert_eq!(pick(Some(older), shipped.clone()), shipped);
        assert_eq!(pick(Some(same.clone()), shipped), same);
    }

    fn two() -> Catalog {
        parse(br#"{"version": 1, "helpers": [
            {"id": "claude", "name": "Claude Code", "command": "claude", "task": {"argv": ["claude", "{prompt}"]}},
            {"id": "codex", "name": "Codex", "command": "codex", "task": {"argv": ["codex", "{prompt}"]}},
            {"id": "pi", "name": "pi", "command": "pi", "task": {"argv": ["pi", "--flag", "{prompt}"]}}
        ]}"#)
        .unwrap()
    }

    #[test]
    fn detection_with_a_fake_path() {
        let dir = scratch("helpers");
        let (bin, home) = (dir.join("bin"), dir.join("home"));
        program(&bin, "codex", "#!/bin/sh\n");
        // Not executable: not installed.
        std::fs::write(bin.join("claude"), "").unwrap();
        let found = installed_in(&two(), bin.to_str().unwrap(), &home, None);
        assert_eq!(found.iter().map(|i| i.helper.id.as_str()).collect::<Vec<_>>(), ["codex"]);
        // ~/.local/bin and ~/.npm-global/bin count; mise answers for the rest.
        program(&home.join(".npm-global/bin"), "claude", "#!/bin/sh\n");
        let pi = program(&dir.join("mise-installs"), "pi", "#!/bin/sh\n");
        let mise = program(&dir, "mise", &format!("#!/bin/sh\n[ \"$1\" = which ] && [ \"$2\" = pi ] && echo {} && exit 0\nexit 1\n", pi.display()));
        let found = installed_in(&two(), &format!("{}:/nonexistent", bin.display()), &home, Some(&mise));
        assert_eq!(found.iter().map(|i| i.helper.id.as_str()).collect::<Vec<_>>(), ["claude", "codex", "pi"]);
        assert_eq!(found[0].program, home.join(".npm-global/bin/claude"));
        assert_eq!(found[2].program, pi);
        // auto = the first; an id when installed; unknown or missing → auto.
        assert_eq!(choose("auto", &found).unwrap().helper.id, "claude");
        assert_eq!(choose("pi", &found).unwrap().helper.id, "pi");
        assert_eq!(choose("hermes", &found).unwrap().helper.id, "claude");
        assert_eq!(choose("codex", &found[..1]).unwrap().helper.id, "claude");
        assert_eq!(choose("auto", &[]), None);
        assert!(installed_in(&two(), "/nonexistent", &home.join("none"), None).is_empty());
        assert!(valid_setting("auto") && valid_setting("cursor-agent") && !valid_setting("../x") && !valid_setting("") && !valid_setting("-x"));
    }

    #[test]
    fn argv_substitution() {
        let installed = Installed { helper: two().helpers[2].clone(), program: "/opt/pi/bin/pi".into() };
        let odd = "$(rm -rf ~); `x` \"q\" 'y' {prompt}\nnext";
        assert_eq!(task_argv(&installed, odd), vec!["/opt/pi/bin/pi", "--flag", odd]);
        assert_eq!(task_argv(&installed, "--help")[2], " --help");
        assert_eq!(task_argv(&installed, "a\0b")[2], "ab");
        let inline = Installed {
            helper: Helper { id: "x".into(), name: "x".into(), command: "x".into(), task: vec!["x".into(), "--p={prompt}".into(), "{prompt}".into()] },
            program: "/x".into(),
        };
        assert_eq!(task_argv(&inline, "-a b"), vec!["/x", "--p=-a b", " -a b"]);
    }
}
