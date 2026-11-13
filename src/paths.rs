//! Where the files live: `$XDG_CONFIG_HOME/maprisma-notifier/settings.json` and
//! `$XDG_STATE_HOME/maprisma-notifier/history.jsonl` (docs/plan.md, the contract the Manager and
//! Maprisma Shell rely on). Tests point both roots at a temporary folder with [use_roots], so they
//! never read or write the session's files.

use std::path::{Path, PathBuf};
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

/// The do not disturb state the recorder last wrote to the desktop (`true`/`false`; COSMIC and
/// Hyprland), so a start can tell a schedule change while it was not running from a change the
/// user made on the desktop.
pub fn dnd_applied_file() -> PathBuf {
    state_root().join(DIR).join("dnd-applied")
}

/// Maprisma Shell's settings (Hyprland), `notifications.dnd`.
pub fn shell_file() -> PathBuf {
    config_root().join("maprisma-shell").join("shell.json")
}

/// COSMIC's notification daemon, cosmic-config key `do_not_disturb`.
pub fn cosmic_dnd_file() -> PathBuf {
    config_root().join("cosmic/com.system76.CosmicNotifications/v1/do_not_disturb")
}

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename. The
/// temporary file is created new (O_EXCL, never an existing file or link) with the usual mode
/// (`0666` less the umask).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_mode(path, bytes, 0o666)
}

/// [write_atomic] for a private file: the temporary file is `0600` from its creation on, so the
/// contents are never readable by others, not even while it is written.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_mode(path, bytes, 0o600)
}

fn write_atomic_mode(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    // A name no other writer (thread, process) uses; a leftover of a crashed one is skipped.
    let (temporary, mut file) = {
        let mut tries = 0;
        loop {
            let candidate = path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&candidate) {
                Ok(file) => break (candidate, file),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
                Err(e) => return Err(e),
            }
        }
    };
    let result = (|| {
        if mode & 0o077 == 0 {
            // Exactly the private mode, through the descriptor (the umask can only take bits away).
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        // The rename itself on disk.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// The lock file of a JSON file changed by [update_json]: `<file name>.lock` in the same folder.
pub fn lock_file(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    path.with_file_name(format!("{name}.lock"))
}

/// An exclusive lock on [lock_file] of a JSON file, held until dropped. It is an open file
/// description lock (`fcntl F_OFD_SETLKW`, the whole file): it excludes other threads of this
/// process too, and it conflicts with the classic POSIX record locks other writers can take
/// (Dart's `RandomAccessFile.lock(FileLock.blockingExclusive)`, Python's `fcntl.lockf`), see
/// docs/plan.md "Writing the settings".
pub struct JsonLock(#[allow(dead_code)] std::fs::File);

impl JsonLock {
    pub fn take(path: &Path) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let lock = lock_file(path);
        if let Some(parent) = lock.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).custom_flags(libc::O_CLOEXEC).open(&lock)?;
        // SAFETY: a zeroed flock is a valid "whole file" range; fcntl only reads it.
        let mut range: libc::flock = unsafe { std::mem::zeroed() };
        range.l_type = libc::F_WRLCK as _;
        range.l_whence = libc::SEEK_SET as _;
        loop {
            // SAFETY: fcntl on a descriptor this File owns, with a valid flock.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLKW, &range) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Changes some keys of a JSON object file and keeps every other key (read–modify–write,
/// atomic), holding [JsonLock] throughout so concurrent writers don't lose each other's changes.
/// A missing or broken file starts as `{}`.
pub fn update_json(path: &Path, change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) -> std::io::Result<()> {
    let _lock = JsonLock::take(path)?;
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

    #[test]
    fn private_files_are_0600_and_no_temporary_is_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_roots().join("state/private-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.jsonl");
        // SAFETY: umask only changes this process's file creation mask.
        let old = unsafe { libc::umask(0o022) };
        write_private(&path, b"secret\n").unwrap();
        write_private(&path, b"secret 2\n").unwrap();
        write_atomic(&dir.join("plain.json"), b"{}").unwrap();
        unsafe { libc::umask(old) };
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"secret 2\n");
        assert_eq!(std::fs::metadata(dir.join("plain.json")).unwrap().permissions().mode() & 0o777, 0o644);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().ends_with(".tmp")).collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn concurrent_updates_keep_every_change() {
        let path = test_roots().join("config/lock-test.json");
        let _ = std::fs::remove_file(&path);
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for round in 0..10 {
                        update_json(&path, |o| {
                            o.insert(format!("k{i}-{round}"), i.into());
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 160);
        assert!(lock_file(&path).exists());
        assert_eq!(lock_file(&path).file_name().unwrap(), "lock-test.json.lock");
    }
}
