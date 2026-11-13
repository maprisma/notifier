//! `history.jsonl` (docs/plan.md): one JSON object per line, appended; compacted (oldest dropped)
//! when over `history_max` entries, older than `history_days` or over [MAX_FILE] bytes. Changes
//! (read, closed, cleared) rewrite the file atomically; every change holds an flock on
//! `history.lock`, so the recorder and the window never lose each other's writes.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAX_FILE: usize = 4 * 1024 * 1024;
pub const MAX_BODY: usize = 2000;
pub const MAX_SUMMARY: usize = 500;
pub const MAX_NAME: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub time: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "normal")]
    pub urgency: u8,
    #[serde(default)]
    pub server_id: u32,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub closed: bool,
    /// A crash report (app "Crashes", see crash.rs): what Details and Check with AI need.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crash: Option<crate::crash::Crash>,
}

fn normal() -> u8 {
    1
}

// ------------------------------------------------------------------ time

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// "2026-11-13T08:12:03Z" (UTC).
pub fn format_utc(unix: i64) -> String {
    let (days, secs) = (unix.div_euclid(86400), unix.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The Unix time of "YYYY-MM-DDTHH:MM:SSZ"; anything else is None.
pub fn parse_utc(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| text.get(r)?.parse::<u32>().ok();
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y as i64, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64)
}

/// A new entry id: time, process and a counter in hex ("uuid-ish", unique enough per user).
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}-{:08x}-{:04x}", nanos as u64, std::process::id(), n & 0xffff)
}

// ------------------------------------------------------------------ text

/// Notification body markup (a subset of HTML: b, i, u, a, img) as plain text: tags removed,
/// `<br>` as a line break, entities decoded, runs of blank lines folded.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(['<', '&']) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with('<') {
            match rest.find('>') {
                Some(end) => {
                    let tag = rest[1..end].trim().trim_start_matches('/').to_ascii_lowercase();
                    if tag.starts_with("br") || tag == "p" {
                        out.push('\n');
                    }
                    rest = &rest[end + 1..];
                }
                None => {
                    out.push('<');
                    rest = &rest[1..];
                }
            }
        } else {
            let end = rest.find(';').filter(|e| *e <= 10);
            let decoded = end.and_then(|e| entity(&rest[1..e]));
            match (end, decoded) {
                (Some(e), Some(c)) => {
                    out.push(c);
                    rest = &rest[e + 1..];
                }
                _ => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        }
    }
    out.push_str(rest);
    let mut folded = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
        blank = if line.is_empty() { blank + 1 } else { 0 };
        if blank <= 1 {
            folded.push_str(line);
            folded.push('\n');
        }
    }
    folded.trim().to_owned()
}

/// Terminal control sequences removed (a program's terminal output sent as a notification: OSC
/// colour queries like `ESC ]10;rgb:…ESC \`, CSI colours like `ESC [1;31m`, charset switches),
/// and every other control character but line breaks and tabs.
pub fn strip_terminal(text: &str) -> String {
    fn string_until_st(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        while let Some(c) = chars.next() {
            match c {
                '\u{7}' | '\u{9c}' => break,
                '\u{1b}' => {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    fn csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        for c in chars.by_ref() {
            if ('@'..='~').contains(&c) {
                break;
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => csi(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => string_until_st(&mut chars),
                // ESC, intermediates (' '..'/'), one final character: `ESC ( B`, `ESC 7` …
                Some(i) if (' '..='/').contains(&i) => {
                    for c in chars.by_ref() {
                        if !(' '..='/').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\u{9b}' => csi(&mut chars),
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => string_until_st(&mut chars),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code).filter(|c| !c.is_control() || *c == '\n')
        }
    }
}

/// At most `limit` characters (an ellipsis marks the cut).
pub fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ------------------------------------------------------------------ the list

/// Drops what is older than `days`, then the oldest beyond `max`, then the oldest until the file
/// fits in MAX_FILE. Entries are in file order (oldest first).
pub fn compact(entries: &mut Vec<Entry>, max: u32, days: u32, now: i64) -> bool {
    let before = entries.len();
    let oldest = now - days as i64 * 86400;
    entries.retain(|e| parse_utc(&e.time).is_none_or(|t| t >= oldest));
    if entries.len() > max as usize {
        entries.drain(..entries.len() - max as usize);
    }
    let mut size: usize = entries.iter().map(|e| line(e).len()).sum();
    let mut drop = 0;
    while size > MAX_FILE && drop < entries.len() {
        size -= line(&entries[drop]).len();
        drop += 1;
    }
    entries.drain(..drop);
    entries.len() != before
}

fn line(entry: &Entry) -> String {
    let mut text = serde_json::to_string(entry).unwrap_or_default();
    text.push('\n');
    text
}

pub fn parse_lines(bytes: &[u8]) -> Vec<Entry> {
    // Entries stored before strip_terminal existed are cleaned as they are read.
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .map(|mut e| {
            if e.summary.contains(|c: char| c.is_control()) {
                e.summary = strip_terminal(&e.summary).replace(['\n', '\t'], " ").trim().to_owned();
            }
            if e.body.contains(|c: char| c.is_control() && c != '\n' && c != '\t') {
                e.body = strip_terminal(&e.body).trim().to_owned();
            }
            e
        })
        .collect()
}

pub fn load_from(path: &Path) -> Vec<Entry> {
    std::fs::read(path).map(|b| parse_lines(&b)).unwrap_or_default()
}

pub fn load() -> Vec<Entry> {
    load_from(&crate::paths::history_file())
}

/// Not read and not from a muted app.
pub fn unread_count(entries: &[Entry], muted: &[String]) -> usize {
    entries.iter().filter(|e| !e.read && !muted.contains(&e.app)).count()
}

/// Groups by app, newest first inside a group, the group with the newest entry first; hidden
/// apps are left out.
pub fn by_app<'a>(entries: &'a [Entry], hidden: &[String]) -> Vec<(String, Vec<&'a Entry>)> {
    let mut groups: Vec<(String, Vec<&Entry>)> = vec![];
    for entry in entries.iter().rev().filter(|e| !hidden.contains(&e.app)) {
        match groups.iter_mut().find(|(app, _)| *app == entry.app) {
            Some((_, list)) => list.push(entry),
            None => groups.push((entry.app.clone(), vec![entry])),
        }
    }
    groups
}

/// Holds `history.lock` (flock) until dropped.
struct Locked(std::fs::File);

impl Locked {
    fn take() -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let path = crate::paths::history_lock();
        std::fs::create_dir_all(path.parent().unwrap())?;
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        // SAFETY: flock on a descriptor this File owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is still open (closed after this).
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn write_all(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let bytes: String = entries.iter().map(line).collect();
    crate::paths::write_private(path, bytes.as_bytes())?;
    set_private(path);
    Ok(())
}

// Notifications can be private: the history is the user's alone.
fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

/// Changes the history under the lock: `change` gets all entries and says whether it changed
/// anything (only then the file is rewritten). Returns what `change` returned.
pub fn modify(change: impl FnOnce(&mut Vec<Entry>) -> bool) -> std::io::Result<bool> {
    let _lock = Locked::take()?;
    let path = crate::paths::history_file();
    let mut entries = load_from(&path);
    let changed = change(&mut entries);
    if changed {
        write_all(&path, &entries)?;
    }
    Ok(changed)
}

/// Adds an entry (the recorder): appended, or the file rewritten when compaction dropped some.
pub fn append(entry: Entry, settings: &crate::settings::Settings) -> std::io::Result<()> {
    use std::io::Write;
    let _lock = Locked::take()?;
    let path = crate::paths::history_file();
    let mut entries = load_from(&path);
    let new_line = line(&entry);
    entries.push(entry);
    if compact(&mut entries, settings.history_max, settings.history_days, crate::settings::now_unix()) {
        return write_all(&path, &entries);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(libc::O_CLOEXEC).open(&path)?
    };
    set_private(&path);
    file.write_all(new_line.as_bytes())
}

/// A notification with `replaces_id` updates that one (newest entry with this server id and app).
pub fn replace(server_id: u32, app: &str, new: Entry) -> std::io::Result<bool> {
    modify(|entries| match entries.iter_mut().rev().find(|e| e.server_id == server_id && e.app == app) {
        Some(old) => {
            let id = std::mem::take(&mut old.id);
            *old = Entry { id, ..new };
            true
        }
        None => false,
    })
}

pub fn mark_read(id: &str) -> std::io::Result<bool> {
    modify(|entries| {
        let mut changed = false;
        for e in entries.iter_mut().filter(|e| e.id == id && !e.read) {
            e.read = true;
            changed = true;
        }
        changed
    })
}

pub fn mark_all_read() -> std::io::Result<bool> {
    modify(|entries| {
        let mut changed = false;
        for e in entries.iter_mut().filter(|e| !e.read) {
            e.read = true;
            changed = true;
        }
        changed
    })
}

pub fn clear_app(app: &str) -> std::io::Result<bool> {
    modify(|entries| {
        let before = entries.len();
        entries.retain(|e| e.app != app);
        entries.len() != before
    })
}

pub fn clear_all() -> std::io::Result<bool> {
    modify(|entries| {
        let changed = !entries.is_empty();
        entries.clear();
        changed
    })
}

/// NotificationClosed / ActionInvoked: the newest open entry with this server id.
pub fn mark_closed(server_id: u32, read: bool) -> std::io::Result<bool> {
    if server_id == 0 {
        return Ok(false);
    }
    modify(|entries| match entries.iter_mut().rev().find(|e| e.server_id == server_id) {
        Some(e) if !e.closed || (read && !e.read) => {
            e.closed = true;
            e.read |= read;
            true
        }
        _ => false,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    /// Tests that use the (temporary) settings or history file run one at a time.
    pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn terminal_sequences_are_not_shown() {
        // What a terminal program's output looked like in a notification body (KDE, COSMIC).
        let osc = "\u{1b}]10;rgb:e0/e0/e0\u{1b}\\\u{1b}]11;rgb:1c/1c/1c\u{7}Build done";
        assert_eq!(strip_terminal(osc), "Build done");
        assert_eq!(strip_terminal("\u{1b}[1;31merror\u{1b}[0m: x\r\n\tnext"), "error: x\n\tnext");
        assert_eq!(strip_terminal("\u{1b}(Bplain \u{9b}2Jtext\u{9d}0;title\u{9c}!"), "plain text!");
        assert_eq!(strip_terminal("unterminated \u{1b}]0;never ends"), "unterminated ");
        assert_eq!(strip_terminal("Grüße – ok"), "Grüße – ok");
        // Stored before the fix: cleaned when read.
        let mut e = entry("Local System", 0, false);
        e.summary = "Hi\u{1b}[0m".into();
        e.body = format!("{osc}\nmore");
        let line = serde_json::to_string(&e).unwrap();
        let back = parse_lines(line.as_bytes());
        assert_eq!(back[0].summary, "Hi");
        assert_eq!(back[0].body, "Build done\nmore");
    }

    pub fn entry(app: &str, time: i64, read: bool) -> Entry {
        Entry {
            id: new_id(),
            time: format_utc(time),
            app: app.into(),
            icon: String::new(),
            summary: format!("{app} {time}"),
            body: String::new(),
            urgency: 1,
            server_id: 0,
            read,
            closed: false,
            crash: None,
        }
    }

    #[test]
    fn utc_round_trip() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_794_557_523), "2026-11-13T08:12:03Z");
        assert_eq!(parse_utc("2026-11-13T08:12:03Z"), Some(1_794_557_523));
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        for t in [0, 951_782_400, 1_794_557_523, 4_102_444_799] {
            assert_eq!(parse_utc(&format_utc(t)), Some(t));
        }
        for bad in ["", "2026-11-13 08:12:03Z", "2026-13-13T08:12:03Z", "2026-11-13T08:12:03", "x026-11-13T08:12:03Z"] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn markup_is_stripped() {
        assert_eq!(strip_markup("<b>Hello</b> <i>world</i>"), "Hello world");
        assert_eq!(strip_markup("a &amp; b &lt;c&gt; &quot;d&quot; &#65;&#x42;"), "a & b <c> \"d\" AB");
        assert_eq!(strip_markup("line<br/>next<br>last"), "line\nnext\nlast");
        assert_eq!(strip_markup("<a href=\"https://x\">link</a> <img src=\"x.png\" alt=\"pic\"/>"), "link");
        assert_eq!(strip_markup("1 < 2 & 3"), "1 < 2 & 3");
        assert_eq!(strip_markup("a\n\n\n\nb  "), "a\n\nb");
        assert_eq!(strip_markup("&bogus; &#0;"), "&bogus; &#0;");
    }

    #[test]
    fn caps() {
        assert_eq!(cap("short", 10), "short");
        let long = "ä".repeat(3000);
        let capped = cap(&long, MAX_BODY);
        assert_eq!(capped.chars().count(), MAX_BODY);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn compaction() {
        let now = 1_794_557_523;
        let mut list: Vec<Entry> = (0..10).map(|i| entry("A", now - (9 - i) * 86400, false)).collect();
        // Older than 3 days dropped (the entries of 4..9 days ago).
        assert!(compact(&mut list, 500, 3, now));
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].time, format_utc(now - 3 * 86400));
        // Over max: the oldest go.
        assert!(compact(&mut list, 2, 90, now));
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].time, format_utc(now));
        assert!(!compact(&mut list, 2, 90, now));
        // Size cap: big entries until the file fits in 4 MB.
        let mut big: Vec<Entry> = (0..3000).map(|i| Entry { body: "x".repeat(MAX_BODY), ..entry("B", now - 3000 + i, false) }).collect();
        assert!(compact(&mut big, 5000, 90, now));
        let size: usize = big.iter().map(|e| line(e).len()).sum();
        assert!(size <= MAX_FILE && size > MAX_FILE - 3000, "{size}");
        assert_eq!(big.last().unwrap().time, format_utc(now - 1));
    }

    #[test]
    fn unread_and_groups() {
        let list = vec![entry("A", 1, false), entry("B", 2, false), entry("A", 3, true), entry("C", 4, false), entry("Muted", 5, false)];
        assert_eq!(unread_count(&list, &[]), 4);
        assert_eq!(unread_count(&list, &["Muted".into()]), 3);
        let groups = by_app(&list, &["C".into()]);
        let names: Vec<&str> = groups.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(names, ["Muted", "A", "B"]);
        assert_eq!(groups[1].1[0].time, format_utc(3));
        assert_eq!(groups[1].1[1].time, format_utc(1));
    }

    #[test]
    fn broken_lines_are_skipped() {
        let good = serde_json::to_string(&entry("A", 1, false)).unwrap();
        let text = format!("{good}\nnot json\n{{\"id\": 1}}\n{{\"id\":\"x\",\"time\":\"t\"}}\n");
        let list = parse_lines(text.as_bytes());
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].urgency, 1);
        assert_eq!(list[1].app, "");
    }

    #[test]
    fn file_changes() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let path = crate::paths::history_file();
        let _ = std::fs::remove_file(&path);
        let settings = crate::settings::Settings { history_max: 50, ..Default::default() };
        let now = crate::settings::now_unix();
        for i in 0..3 {
            append(Entry { server_id: 10 + i as u32, ..entry(if i == 2 { "B" } else { "A" }, now - 10 + i, false) }, &settings).unwrap();
        }
        assert_eq!(load().len(), 3);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // Over history_max: the oldest dropped by the rewrite.
        for i in 0..60 {
            append(entry("C", now - 5 + i % 5, false), &settings).unwrap();
        }
        let list = load();
        assert_eq!(list.len(), 50);
        assert!(list.iter().all(|e| e.app == "C"));
        clear_all().unwrap();
        assert!(load().is_empty());
        for i in 0..3 {
            append(Entry { server_id: 10 + i as u32, ..entry(if i == 2 { "B" } else { "A" }, now - 10 + i, false) }, &settings).unwrap();
        }
        let first = load()[0].id.clone();
        assert!(mark_read(&first).unwrap());
        assert!(!mark_read(&first).unwrap());
        assert_eq!(unread_count(&load(), &[]), 2);
        assert!(mark_closed(12, false).unwrap());
        assert!(load()[2].closed && !load()[2].read);
        assert!(mark_closed(12, true).unwrap());
        assert!(load()[2].read);
        assert!(!mark_closed(99, true).unwrap());
        assert!(replace(11, "A", Entry { summary: "new".into(), ..entry("A", now, false) }).unwrap());
        assert_eq!(load()[1].summary, "new");
        assert!(!replace(11, "Other", entry("A", now, false)).unwrap());
        assert!(mark_all_read().unwrap());
        assert_eq!(unread_count(&load(), &[]), 0);
        assert!(clear_app("A").unwrap());
        assert_eq!(load().len(), 1);
        assert!(clear_all().unwrap());
        assert!(!clear_all().unwrap());
    }
}
