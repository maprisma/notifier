//! F12 screenshots of the popup or window, saved like AI Playground's and the Omarchy panels'
//! (shot_save.py in the author's plugin kit). Shared copy: the same file in every Maprisma
//! COSMIC app; change it in all of them.
//!
//! The picture is the app's own surface, drawn again by iced (`window::screenshot`): RGBA in
//! memory, encoded here as PNG. It goes to ~/Pictures/<App name>/ (XDG_PICTURES_DIR from
//! user-dirs.dirs when it is set there), a folder made 0700 when missing and used only when it is
//! a real folder (not a link) owned by this user that group and others cannot write to. Files are
//! created relative to that folder handle, never through a link and never over an existing file
//! (0600, written completely, fsync'd, removed again when writing fails); a name whose note (.txt)
//! exists is skipped too. A note is at most NOTE_LIMIT characters; no text, no file.
//! Nothing is sent anywhere and no other program is started.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const NOTE_LIMIT: usize = 4000; // characters: a few sentences, not a document
const USER_DIRS_LIMIT: usize = 64 * 1024; // bytes of user-dirs.dirs read at most
const MAX_SIDE: u32 = 8192; // pixels per side of a picture at most
const MAX_PIXELS: u64 = 40_000_000; // pixels of a picture at most

/// A saved file: its path and the path as shown in the popup (~ for the home folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    pub shown: String,
}

fn error(text: &str) -> io::Error {
    io::Error::other(text.to_string())
}

fn cstring(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| error("not a usable file name"))
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

// ------------------------------------------------------------------ folders

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

/// The bytes of a small regular file: opened without following a link and without blocking (a
/// FIFO), checked with fstat, at most `limit` bytes; anything else is None.
fn read_small_regular(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let name = cstring(path.as_os_str()).ok()?;
    let fd = unsafe {
        libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut info) } != 0 || info.st_mode & libc::S_IFMT != libc::S_IFREG {
        return None;
    }
    let mut data = vec![0u8; limit + 1];
    let mut filled = 0;
    while filled < data.len() {
        let n = unsafe { libc::read(fd.as_raw_fd(), data[filled..].as_mut_ptr().cast(), data.len() - filled) };
        if n < 0 {
            if last_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if n == 0 {
            break;
        }
        filled += n as usize;
    }
    if filled > limit {
        return None;
    }
    data.truncate(filled);
    Some(data)
}

/// XDG_PICTURES_DIR from `<config>/user-dirs.dirs` (only "$HOME/…", "$HOME" or an absolute
/// path), else `<home>/Pictures`.
pub fn pictures_dir_in(home: &Path, config: &Path) -> PathBuf {
    let text = read_small_regular(&config.join("user-dirs.dirs"), USER_DIRS_LIMIT)
        .map(|data| String::from_utf8_lossy(&data).into_owned())
        .unwrap_or_default();
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("XDG_PICTURES_DIR") else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let rest = rest.trim();
        let Some(value) = rest.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else { continue };
        if value.contains('"') {
            continue;
        }
        let value = if value == "$HOME" {
            home.to_path_buf()
        } else if let Some(sub) = value.strip_prefix("$HOME/") {
            home.join(sub)
        } else {
            PathBuf::from(value)
        };
        if value.is_absolute() && value.components().count() > 1 {
            found = Some(value.components().collect::<PathBuf>());
        }
    }
    found.unwrap_or_else(|| home.join("Pictures"))
}

pub fn pictures_dir() -> PathBuf {
    pictures_dir_in(&home(), &config_home())
}

fn check_folder_name(name: &str) -> io::Result<&str> {
    if name.is_empty()
        || name.chars().count() > 64
        || name.starts_with('.')
        || name.contains('/')
        || name.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
    {
        return Err(error("not a usable folder name"));
    }
    Ok(name)
}

/// The path with ~ for the home folder, as the popup shows it.
pub fn shown_in(path: &Path, home: &Path) -> String {
    let base = home.to_string_lossy();
    let base = base.trim_end_matches('/');
    let text = path.to_string_lossy().into_owned();
    if base.is_empty() {
        return text;
    }
    if text == base {
        return "~".into();
    }
    match text.strip_prefix(&format!("{base}/")) {
        Some(rest) => format!("~/{rest}"),
        None => text,
    }
}

/// A handle on the screenshot folder: made 0700 when missing, and used only when it is a real
/// folder (not a link) owned by this user that group and others cannot write to.
pub fn private_folder(where_: &Path) -> io::Result<OwnedFd> {
    let refuse = || error(&format!("{} is not a private folder of yours", shown_in(where_, &home())));
    if let Some(parent) = where_.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = cstring(where_.as_os_str())?;
    if unsafe { libc::mkdir(name.as_ptr(), 0o700) } != 0 && last_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(last_error());
    }
    let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(refuse());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut info) } != 0 {
        return Err(last_error());
    }
    if info.st_mode & libc::S_IFMT != libc::S_IFDIR || info.st_uid != unsafe { libc::getuid() } || info.st_mode & 0o022 != 0 {
        return Err(refuse());
    }
    Ok(fd)
}

// ------------------------------------------------------------------ files

/// Whether `name` exists in the folder (as anything, a link too).
fn taken(dir: RawFd, name: &str) -> io::Result<bool> {
    let name = cstring(OsStr::new(name))?;
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) } == 0 {
        return Ok(true);
    }
    let error = last_error();
    if error.kind() == io::ErrorKind::NotFound { Ok(false) } else { Err(error) }
}

/// The app's name as a file name prefix: lower case, other characters as "-", at most 40.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(40).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "screenshot".into() } else { out }
}

/// `<prefix>-<stamp>.png`, or -2, -3 … when it or its note already exists in the folder.
pub fn free_name(dir: RawFd, prefix: &str, stamp: &str) -> io::Result<String> {
    let mut name = format!("{prefix}-{stamp}.png");
    let mut n = 2;
    while taken(dir, &name)? || taken(dir, &format!("{}.txt", &name[..name.len() - 4]))? {
        name = format!("{prefix}-{stamp}-{n}.png");
        n += 1;
    }
    Ok(name)
}

/// Writes all of `data` with `write` (which may write less than asked); a write of 0 bytes fails
/// instead of spinning, an interrupted one is tried again.
pub fn write_all_with(data: &[u8], write: &mut dyn FnMut(&[u8]) -> io::Result<usize>) -> io::Result<()> {
    let mut rest = data;
    while !rest.is_empty() {
        match write(rest) {
            Ok(0) => return Err(error("the file could not be written")),
            Ok(n) => rest = &rest[n.min(rest.len())..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn write_fd(fd: RawFd, data: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
    if n < 0 { Err(last_error()) } else { Ok(n as usize) }
}

/// A new file of our own in the folder (O_EXCL, never through a link, 0600), written completely
/// and fsync'd, or not left behind at all. `write` writes to the open file (tests shorten it).
pub fn write_new_with(
    dir: RawFd,
    name: &str,
    data: &[u8],
    write: &mut dyn FnMut(RawFd, &[u8]) -> io::Result<usize>,
) -> io::Result<()> {
    let cname = cstring(OsStr::new(name))?;
    let fd = unsafe {
        libc::openat(
            dir,
            cname.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(last_error());
    }
    let file = unsafe { OwnedFd::from_raw_fd(fd) };
    let raw = file.as_raw_fd();
    let result = write_all_with(data, &mut |chunk| write(raw, chunk)).and_then(|_| {
        if unsafe { libc::fsync(raw) } != 0 { Err(last_error()) } else { Ok(()) }
    });
    drop(file);
    if result.is_err() {
        unsafe { libc::unlinkat(dir, cname.as_ptr(), 0) };
    }
    result
}

pub fn write_new(dir: RawFd, name: &str, data: &[u8]) -> io::Result<()> {
    write_new_with(dir, name, data, &mut write_fd)
}

/// Local time as YYYYMMDD-HHMMSS.
pub fn stamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Saves a PNG as `<pictures>/<folder name>/<slug>-<stamp>[-n].png`.
pub fn save_picture_in(pictures: &Path, home: &Path, folder_name: &str, png: &[u8], stamp: &str) -> io::Result<Saved> {
    let where_ = pictures.join(check_folder_name(folder_name)?);
    let dir = private_folder(&where_)?;
    let name = free_name(dir.as_raw_fd(), &slug(folder_name), stamp)?;
    write_new(dir.as_raw_fd(), &name, png)?;
    let path = where_.join(name);
    Ok(Saved { shown: shown_in(&path, home), path })
}

pub fn save_picture(folder_name: &str, png: &[u8]) -> io::Result<Saved> {
    save_picture_in(&pictures_dir(), &home(), folder_name, png, &stamp())
}

fn picture_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else { return false };
    let mut chars = stem.chars();
    stem.len() <= 81
        && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The note text as it is saved: trimmed, at most NOTE_LIMIT characters; None when empty.
pub fn note_text(text: &str) -> Option<String> {
    let text: String = text.trim().chars().take(NOTE_LIMIT).collect();
    let text = text.trim_end().to_string();
    (!text.is_empty()).then_some(text)
}

/// Saves the note next to `picture` (same name, .txt) in `<pictures>/<folder name>`; no text,
/// no file (Ok(None)).
pub fn save_note_in(pictures: &Path, home: &Path, picture: &Path, folder_name: &str, text: &str) -> io::Result<Option<Saved>> {
    let where_ = pictures.join(check_folder_name(folder_name)?);
    let name = picture.file_name().and_then(OsStr::to_str).unwrap_or("");
    if picture.parent() != Some(where_.as_path()) || !picture_name(name) {
        return Err(error("the picture is not in the screenshot folder"));
    }
    let Some(text) = note_text(text) else { return Ok(None) };
    let dir = private_folder(&where_)?;
    let cname = cstring(OsStr::new(name))?;
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir.as_raw_fd(), cname.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(error("the picture is gone"));
    }
    if info.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(error("the picture is not a file"));
    }
    let note = format!("{}.txt", &name[..name.len() - 4]);
    write_new(dir.as_raw_fd(), &note, format!("{text}\n").as_bytes())?;
    let path = where_.join(note);
    Ok(Some(Saved { shown: shown_in(&path, home), path }))
}

pub fn save_note(picture: &Path, folder_name: &str, text: &str) -> io::Result<Option<Saved>> {
    save_note_in(&pictures_dir(), &home(), picture, folder_name, text)
}

// ------------------------------------------------------------------ picture

/// iced's screenshot as a PNG. Both renderers hand over premultiplied colours, so the
/// see-through edges (rounded corners) are turned back into straight alpha first.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> io::Result<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(error("the picture is too large"));
    }
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(error("the picture could not be drawn"));
    }
    let mut pixels = rgba.to_vec();
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3];
        if a != 0 && a != 255 && px[..3].iter().all(|c| *c <= a) {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| error(&e.to_string()))?;
        writer.write_image_data(&pixels).map_err(|e| error(&e.to_string()))?;
    }
    Ok(out)
}

/// Encodes and saves in one go (run off the UI thread).
pub fn save_screenshot(folder_name: &str, rgba: &[u8], width: u32, height: u32) -> Result<Saved, String> {
    let png = encode_png(rgba, width, height).map_err(|e| e.to_string())?;
    save_picture(folder_name, &png).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ the popup's state

/// What the F12 note shows: nothing, a capture being saved, the note field, the note being
/// written, or an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Busy,
    Note { path: PathBuf, shown: String, text: String },
    Noting,
    Error(String),
}

impl Phase {
    /// The overlay (note or error) is showing.
    pub fn open(&self) -> bool {
        matches!(self, Phase::Note { .. } | Phase::Error(_))
    }
}

/// The F12 key: no Ctrl, Shift, Alt or Super.
pub fn is_shot_key(key: &cosmic::iced::keyboard::Key, modifiers: cosmic::iced::keyboard::Modifiers) -> bool {
    matches!(key, cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::F12)) && modifiers.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shot-test-{}-{tag}-{}", std::process::id(), stamp()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_private_files_and_never_overwrites() {
        let root = scratch("save");
        let pictures = root.join("Pictures");
        let first = save_picture_in(&pictures, &root, "Maprisma Test", b"one", "20261110-120000").unwrap();
        assert_eq!(first.path, pictures.join("Maprisma Test/maprisma-test-20261110-120000.png"));
        assert_eq!(first.shown, "~/Pictures/Maprisma Test/maprisma-test-20261110-120000.png");
        let folder = std::fs::metadata(pictures.join("Maprisma Test")).unwrap();
        assert_eq!(folder.permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&first.path).unwrap().permissions().mode() & 0o777, 0o600);
        let second = save_picture_in(&pictures, &root, "Maprisma Test", b"two", "20261110-120000").unwrap();
        assert!(second.path.ends_with("maprisma-test-20261110-120000-2.png"));
        assert_eq!(std::fs::read(&first.path).unwrap(), b"one");
        // A note with the next name blocks that name too.
        std::fs::write(pictures.join("Maprisma Test/maprisma-test-20261110-120000-3.txt"), "x").unwrap();
        let third = save_picture_in(&pictures, &root, "Maprisma Test", b"three", "20261110-120000").unwrap();
        assert!(third.path.ends_with("maprisma-test-20261110-120000-4.png"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_linked_or_foreign_writable_folders() {
        let root = scratch("folders");
        let pictures = root.join("Pictures");
        std::fs::create_dir_all(&pictures).unwrap();
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, pictures.join("Linked")).unwrap();
        assert!(save_picture_in(&pictures, &root, "Linked", b"x", "20261110-120000").is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
        let open = pictures.join("Open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(save_picture_in(&pictures, &root, "Open", b"x", "20261110-120000").is_err());
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o720)).unwrap();
        assert!(save_picture_in(&pictures, &root, "Open", b"x", "20261110-120000").is_err());
        assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0);
        for bad in ["", ".hidden", "a/b", "tab\there"] {
            assert!(save_picture_in(&pictures, &root, bad, b"x", "20261110-120000").is_err(), "{bad:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_link_name_is_skipped_not_followed() {
        let root = scratch("link-name");
        let pictures = root.join("Pictures");
        let folder = pictures.join("App");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("target");
        symlink(&target, folder.join("app-20261110-120000.png")).unwrap();
        let saved = save_picture_in(&pictures, &root, "App", b"x", "20261110-120000").unwrap();
        assert!(saved.path.ends_with("app-20261110-120000-2.png"));
        assert!(!target.exists());
        let dir = private_folder(&folder).unwrap();
        assert!(write_new(dir.as_raw_fd(), "app-20261110-120000.png", b"y").is_err());
        assert!(!target.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_writes_complete_and_zero_writes_fail_without_leftovers() {
        let root = scratch("writes");
        let folder = root.join("App");
        let dir = private_folder(&folder).unwrap();
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let mut calls = 0;
        write_new_with(dir.as_raw_fd(), "short.png", &data, &mut |fd, chunk| {
            calls += 1;
            write_fd(fd, &chunk[..chunk.len().min(7)])
        })
        .unwrap();
        assert!(calls > 100);
        assert_eq!(std::fs::read(folder.join("short.png")).unwrap(), data);
        let mut first = true;
        let zero = write_new_with(dir.as_raw_fd(), "zero.png", &data, &mut |fd, chunk| {
            if std::mem::take(&mut first) { write_fd(fd, &chunk[..10]) } else { Ok(0) }
        });
        assert!(zero.is_err());
        assert!(!folder.join("zero.png").exists());
        let failing = write_new_with(dir.as_raw_fd(), "fail.png", &data, &mut |_, _| Err(io::Error::from_raw_os_error(libc::ENOSPC)));
        assert!(failing.is_err());
        assert!(!folder.join("fail.png").exists());
        // An existing file is never written over.
        assert!(write_new(dir.as_raw_fd(), "short.png", b"other").is_err());
        assert_eq!(std::fs::read(folder.join("short.png")).unwrap(), data);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn note_rules() {
        let root = scratch("note");
        let pictures = root.join("Pictures");
        let saved = save_picture_in(&pictures, &root, "App", b"png", "20261110-120000").unwrap();
        assert_eq!(save_note_in(&pictures, &root, &saved.path, "App", "   \n ").unwrap(), None);
        assert!(!saved.path.with_extension("txt").exists());
        let long = "é".repeat(NOTE_LIMIT + 50);
        let note = save_note_in(&pictures, &root, &saved.path, "App", &format!("  {long}  ")).unwrap().unwrap();
        assert_eq!(note.path, saved.path.with_extension("txt"));
        let text = std::fs::read_to_string(&note.path).unwrap();
        assert_eq!(text.chars().count(), NOTE_LIMIT + 1);
        assert!(text.ends_with('\n'));
        assert_eq!(std::fs::metadata(&note.path).unwrap().mode() & 0o777, 0o600);
        // A second note never replaces the first.
        assert!(save_note_in(&pictures, &root, &saved.path, "App", "again").is_err());
        assert_eq!(std::fs::read_to_string(&note.path).unwrap(), text);
        // Only pictures in the screenshot folder, with a picture's name, that still exist.
        assert!(save_note_in(&pictures, &root, &root.join("x.png"), "App", "t").is_err());
        assert!(save_note_in(&pictures, &root, &pictures.join("App/../App/x.png"), "App", "t").is_err());
        assert!(save_note_in(&pictures, &root, &pictures.join("App/Bad Name.png"), "App", "t").is_err());
        assert!(save_note_in(&pictures, &root, &pictures.join("App/gone.png"), "App", "t").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_dirs_only_from_a_small_regular_file() {
        let root = scratch("user-dirs");
        let home = root.join("home");
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(pictures_dir_in(&home, &config), home.join("Pictures"));
        std::fs::write(config.join("user-dirs.dirs"), "# x\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n").unwrap();
        assert_eq!(pictures_dir_in(&home, &config), home.join("Bilder"));
        std::fs::write(config.join("user-dirs.dirs"), "XDG_PICTURES_DIR=\"/srv/pics/\"\n").unwrap();
        assert_eq!(pictures_dir_in(&home, &config), PathBuf::from("/srv/pics"));
        std::fs::write(config.join("user-dirs.dirs"), "XDG_PICTURES_DIR=\"relative\"\nXDG_PICTURES_DIR=\"/\"\n").unwrap();
        assert_eq!(pictures_dir_in(&home, &config), home.join("Pictures"));
        // Too large: ignored.
        let big = format!("XDG_PICTURES_DIR=\"/big\"\n{}", "#".repeat(USER_DIRS_LIMIT));
        std::fs::write(config.join("user-dirs.dirs"), big).unwrap();
        assert_eq!(pictures_dir_in(&home, &config), home.join("Pictures"));
        // A link to a valid file: not followed.
        std::fs::remove_file(config.join("user-dirs.dirs")).unwrap();
        let real = root.join("real.dirs");
        std::fs::write(&real, "XDG_PICTURES_DIR=\"/linked\"\n").unwrap();
        symlink(&real, config.join("user-dirs.dirs")).unwrap();
        assert_eq!(pictures_dir_in(&home, &config), home.join("Pictures"));
        // A FIFO: neither blocks nor counts.
        std::fs::remove_file(config.join("user-dirs.dirs")).unwrap();
        let fifo = CString::new(config.join("user-dirs.dirs").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(pictures_dir_in(&home, &config), home.join("Pictures"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_and_paths() {
        assert_eq!(slug("Maprisma LLM Monitor"), "maprisma-llm-monitor");
        assert_eq!(slug("  ** "), "screenshot");
        assert_eq!(shown_in(Path::new("/home/u/Pictures/x.png"), Path::new("/home/u")), "~/Pictures/x.png");
        assert_eq!(shown_in(Path::new("/home/uwe2/x.png"), Path::new("/home/u")), "/home/uwe2/x.png");
        assert_eq!(stamp().len(), 15);
        assert!(picture_name("app-20261110-120000-2.png"));
        assert!(!picture_name("-a.png") && !picture_name("A.png") && !picture_name("a.txt"));
    }

    #[test]
    fn png_encoding() {
        let png = encode_png(&[255, 0, 0, 255, 64, 0, 0, 128], 2, 1).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
        decoder.set_transformations(png::Transformations::IDENTITY);
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..8], &[255, 0, 0, 255, 128, 0, 0, 128]);
        assert!(encode_png(&[0; 4], 2, 1).is_err());
        assert!(encode_png(&[], 0, 0).is_err());
        assert!(encode_png(&[], MAX_SIDE + 1, 1).is_err());
    }

    #[test]
    fn note_text_and_phase() {
        assert_eq!(note_text(" \t\n"), None);
        assert_eq!(note_text("  wrong colour \n").as_deref(), Some("wrong colour"));
        assert!(Phase::Error("x".into()).open() && !Phase::Busy.open() && !Phase::Idle.open());
        use cosmic::iced::keyboard::{Key, Modifiers, key::Named};
        assert!(is_shot_key(&Key::Named(Named::F12), Modifiers::empty()));
        assert!(!is_shot_key(&Key::Named(Named::F12), Modifiers::CTRL));
        assert!(!is_shot_key(&Key::Named(Named::F11), Modifiers::empty()));
    }
}
