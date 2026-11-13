//! Crash reports (docs/plan.md "Crashes"): the recorder follows systemd-coredump's journal entries
//! (`journalctl -f -o json MESSAGE_ID=…`) for the user's own programs and sends one desktop
//! notification per crash (app "Crashes"), a burst of the same program folded into one. Its
//! actions open a terminal with `coredumpctl info <pid>` ("Details") or an AI helper asked to
//! explain the crash ("Check with AI"). The window offers the same two buttons on crash entries.
//!
//! Everything that decides something is a pure function here, tested without a journal or a bus.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Notify `app_name` of crash notifications (and their app in the history).
pub const APP: &str = "Crashes";
pub const ICON: &str = "dialog-error";
/// systemd-coredump's "process dumped core" message.
pub const MESSAGE_ID: &str = "fc2e22bc6ee647b6b90729ab34a250b1";
pub const ACTION_DETAILS: &str = "details";
pub const ACTION_AI: &str = "ai";
/// Hints carrying the crash to the recorder's own history entry.
pub const HINT_PID: &str = "x-maprisma-crash-pid";
pub const HINT_EXE: &str = "x-maprisma-crash-exe";
pub const HINT_NAME: &str = "x-maprisma-crash-name";
pub const HINT_SIGNAL: &str = "x-maprisma-crash-signal";
pub const HINT_TIME: &str = "x-maprisma-crash-time";
/// The same program crashing again within this updates its notification.
pub const SAME_PROGRAM_MS: u64 = 60_000;
/// At most this many crash notifications (new or updated) per [RATE_WINDOW_MS].
pub const RATE_MAX: usize = 5;
pub const RATE_WINDOW_MS: u64 = 60_000;
const NAME_MAX: usize = 64;
const EXE_MAX: usize = 512;

/// One crash, as the history keeps it (`"crash"` of a "Crashes" entry).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Crash {
    pub pid: u32,
    #[serde(default)]
    pub exe: String,
    /// The name shown: the .desktop Name of the program, else its command name.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub signal: String,
    /// Unix seconds.
    #[serde(default)]
    pub time: i64,
}

/// Printable text only (journal fields come from the crashed program), at most `limit` chars.
pub fn clean(text: &str, limit: usize) -> String {
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    crate::history::cap(text.trim(), limit)
}

/// A journal JSON field: a string, the first of several values, or bytes (non-UTF-8 values are
/// arrays of numbers).
fn field(entry: &Value, key: &str) -> Option<String> {
    match entry.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(list) if !list.is_empty() && list.iter().all(Value::is_u64) => {
            let bytes: Vec<u8> = list.iter().filter_map(Value::as_u64).map(|b| b.min(255) as u8).collect();
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
        Value::Array(list) => list.iter().find_map(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// One line of `journalctl -o json`: the crash, when it is a coredump of `uid`'s program.
/// `now` fills a missing time.
pub fn parse_line(line: &str, uid: u32, now: i64) -> Option<Crash> {
    let entry: Value = serde_json::from_str(line.trim()).ok()?;
    if field(&entry, "MESSAGE_ID").is_some_and(|m| m != MESSAGE_ID) {
        return None;
    }
    if field(&entry, "COREDUMP_UID")?.trim().parse::<u32>().ok()? != uid {
        return None;
    }
    let pid = field(&entry, "COREDUMP_PID")?.trim().parse::<u32>().ok().filter(|p| *p > 0)?;
    let exe = clean(&field(&entry, "COREDUMP_EXE").unwrap_or_default(), EXE_MAX);
    let comm = clean(&field(&entry, "COREDUMP_COMM").unwrap_or_default(), NAME_MAX);
    if exe.is_empty() && comm.is_empty() {
        return None;
    }
    let signal = match field(&entry, "COREDUMP_SIGNAL_NAME").map(|s| clean(&s, NAME_MAX)).filter(|s| !s.is_empty()) {
        Some(name) => name,
        None => field(&entry, "COREDUMP_SIGNAL").and_then(|n| n.trim().parse::<i32>().ok()).map_or_else(|| "?".into(), |n| format!("signal {n}")),
    };
    let time = field(&entry, "__REALTIME_TIMESTAMP")
        .or_else(|| field(&entry, "COREDUMP_TIMESTAMP"))
        .and_then(|t| t.trim().parse::<i64>().ok())
        .map_or(now, |micros| micros / 1_000_000);
    let name = if comm.is_empty() { base_name(&exe).to_owned() } else { comm };
    Some(Crash { pid, exe, name, signal, time })
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ------------------------------------------------------------------ a nicer name

/// Commands a .desktop Exec starts that say nothing about the program behind them.
const LAUNCHERS: [&str; 12] = ["sh", "bash", "zsh", "env", "flatpak", "python", "python3", "perl", "java", "node", "electron", "gjs"];

/// The `Name` of the first application entry in `dirs` whose Exec runs `exe` (its path or its
/// file name); a few hundred small files at most, read only when a program crashed.
pub fn desktop_name(exe: &str, dirs: &[PathBuf]) -> Option<String> {
    let base = base_name(exe);
    if base.is_empty() || LAUNCHERS.contains(&base) {
        return None;
    }
    let mut read = 0;
    for dir in dirs {
        let Ok(list) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = list.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        files.sort();
        for path in files {
            read += 1;
            if read > 3000 {
                return None;
            }
            if std::fs::metadata(&path).map_or(true, |m| !m.is_file() || m.len() > 64 * 1024) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(name) = entry_name_for(&text, exe, base) {
                return Some(name);
            }
        }
    }
    None
}

fn entry_name_for(text: &str, exe: &str, base: &str) -> Option<String> {
    let (mut inside, mut name, mut runs, mut hidden) = (false, None, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(value) = line.strip_prefix("Name=") {
            name = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("Exec=") {
            let program = value.split_whitespace().next().unwrap_or("").trim_matches('"');
            runs = program == exe || (!program.contains('/') && program == base);
        } else if line == "Hidden=true" {
            hidden = true;
        }
    }
    let name = clean(&name?, NAME_MAX);
    (runs && !hidden && !name.is_empty()).then_some(name)
}

/// The application folders, the user's first.
pub fn application_dirs() -> Vec<PathBuf> {
    let home = crate::paths::home();
    let data_home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home).chain(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).map(|d| d.join("applications")).collect()
}

// ------------------------------------------------------------------ bursts

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// A new notification.
    New,
    /// The program's notification again (`replaces_id`), now with `count` crashes.
    Update { replaces_id: u32, count: u32 },
    /// Over the rate limit: nothing sent (a later update still counts this one).
    Drop,
}

struct Recent {
    id: u32,
    count: u32,
    last: u64,
}

/// Folds bursts: the same program again within [SAME_PROGRAM_MS] of its last crash updates its
/// notification; at most [RATE_MAX] sent per [RATE_WINDOW_MS]. Times are milliseconds of any
/// monotonic clock (tests pass their own).
#[derive(Default)]
pub struct Coalescer {
    recent: HashMap<String, Recent>,
    sent: VecDeque<u64>,
}

impl Coalescer {
    pub fn decide(&mut self, exe: &str, now: u64) -> Decision {
        self.recent.retain(|_, r| now.saturating_sub(r.last) < SAME_PROGRAM_MS);
        while self.sent.front().is_some_and(|t| now.saturating_sub(*t) >= RATE_WINDOW_MS) {
            self.sent.pop_front();
        }
        let limited = self.sent.len() >= RATE_MAX;
        let decision = match self.recent.get_mut(exe) {
            Some(recent) => {
                recent.count += 1;
                recent.last = now;
                if limited { Decision::Drop } else { Decision::Update { replaces_id: recent.id, count: recent.count } }
            }
            None if limited => Decision::Drop,
            None => {
                self.recent.insert(exe.to_owned(), Recent { id: 0, count: 1, last: now });
                Decision::New
            }
        };
        if decision != Decision::Drop {
            self.sent.push_back(now);
        }
        decision
    }

    /// The server's id of the notification just sent for `exe`.
    pub fn sent(&mut self, exe: &str, id: u32) {
        if let Some(recent) = self.recent.get_mut(exe)
            && id != 0
        {
            recent.id = id;
        }
    }
}

// ------------------------------------------------------------------ the notification

pub type Hints = HashMap<String, zbus::zvariant::Value<'static>>;

/// Notify's arguments (`susssasa{sv}i`).
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyArgs {
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: Hints,
    pub timeout: i32,
}

/// Body markup: `&`, `<` and `>` as entities (a program path is not markup).
pub fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Local date and time, "2026-11-13 08:12:03".
pub fn local_time(unix: i64) -> String {
    let t = crate::settings::local_tm(unix);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec)
}

pub fn summary(crash: &Crash, count: u32) -> String {
    use crate::i18n::trf;
    if count > 1 { trf("{0} crashed {1} times", &[crash.name.clone(), count.to_string()]) } else { trf("{0} crashed", &[crash.name.clone()]) }
}

/// The notification for a crash (`count` so far, `replaces_id` 0 for a new one); `ai`: a helper
/// is installed, so "Check with AI" is offered.
pub fn notify_args(crash: &Crash, count: u32, replaces_id: u32, ai: bool) -> NotifyArgs {
    use crate::i18n::tr;
    use zbus::zvariant::Value;
    let mut actions = vec![ACTION_DETAILS.to_owned(), tr("Details")];
    if ai {
        actions.extend([ACTION_AI.to_owned(), tr("Check with AI")]);
    }
    let mut hints: Hints = HashMap::new();
    hints.insert("urgency".into(), Value::U8(1));
    hints.insert("category".into(), Value::from("crash".to_owned()));
    hints.insert(HINT_PID.into(), Value::U32(crash.pid));
    hints.insert(HINT_EXE.into(), Value::from(crash.exe.clone()));
    hints.insert(HINT_NAME.into(), Value::from(crash.name.clone()));
    hints.insert(HINT_SIGNAL.into(), Value::from(crash.signal.clone()));
    hints.insert(HINT_TIME.into(), Value::I64(crash.time));
    let lines: Vec<String> = [crash.signal.as_str(), crash.exe.as_str()].into_iter().filter(|s| !s.is_empty()).map(escape_markup).collect();
    NotifyArgs {
        app_name: APP.into(),
        replaces_id,
        app_icon: ICON.into(),
        summary: summary(crash, count),
        body: format!("{}\n{}", lines.join("\n"), local_time(crash.time)),
        actions,
        hints,
        timeout: -1,
    }
}

// ------------------------------------------------------------------ programs

/// An executable regular file named `program` in the `:`-separated `path`.
pub fn which_in(program: &str, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if program.is_empty() || program.contains('/') {
        return None;
    }
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(program)).find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

pub(crate) fn env_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Runs `argv` and returns its first line of output; killed after `limit`.
pub(crate) fn first_line(argv: &[&str], limit: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(argv.first()?)
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.take(64 * 1024).read_to_string(&mut out).ok()?;
                return status.success().then(|| out.lines().next().unwrap_or("").trim().to_owned()).filter(|l| !l.is_empty());
            }
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

// ------------------------------------------------------------------ AI helpers

/// The first prompt for the helper, in the interface's language.
pub fn prompt(crash: &Crash) -> String {
    crate::i18n::trf(
        "{0} ({1}) crashed with {2} at {3}, PID {4}. Read `coredumpctl info {4}` (and use `coredumpctl debug` only if needed), explain the likely cause in plain words and suggest what I can do. Don't change anything on my system without asking me first.",
        &[crash.name.clone(), crash.exe.clone(), crash.signal.clone(), local_time(crash.time), crash.pid.to_string()],
    )
}

/// `terminal… sh -c 'coredumpctl info "$1" 2>&1 | less' sh <pid>`.
pub fn details_argv(terminal: &[String], pid: u32) -> Vec<String> {
    terminal.iter().cloned().chain(["sh".into(), "-c".into(), "coredumpctl info \"$1\" 2>&1 | less".into(), "sh".into(), pid.to_string()]).collect()
}

// ------------------------------------------------------------------ the terminal

/// Known terminals and what comes before the command (Maprisma's desktop_generic list).
pub const TERMINALS: [(&str, &[&str]); 11] = [
    ("kgx", &["--"]),
    ("ptyxis", &["--"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("cosmic-term", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("foot", &[]),
    ("alacritty", &["-e"]),
    ("kitty", &[]),
    ("wezterm", &["start", "--"]),
    ("xterm", &["-e"]),
];

/// The terminal to run a command in: xdg-terminal-exec (when it finds a terminal: `resolves`),
/// else `$TERMINAL` (a plain program name), else the first known one installed.
pub fn terminal_in(path: &str, own: Option<&str>, resolves: impl Fn(&Path) -> bool) -> Option<Vec<String>> {
    if let Some(launcher) = which_in("xdg-terminal-exec", path).filter(|l| resolves(l)) {
        return Some(vec![launcher.to_string_lossy().into_owned()]);
    }
    let plain = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
    if let Some(own) = own.filter(|o| plain(o))
        && let Some(found) = which_in(own, path)
    {
        let follow: &[&str] = TERMINALS.iter().find(|(t, _)| *t == own).map_or(&["-e"], |(_, f)| f);
        return Some(std::iter::once(found.to_string_lossy().into_owned()).chain(follow.iter().map(|s| s.to_string())).collect());
    }
    TERMINALS.iter().find_map(|(program, follow)| {
        which_in(program, path).map(|found| std::iter::once(found.to_string_lossy().into_owned()).chain(follow.iter().map(|s| s.to_string())).collect())
    })
}

pub fn terminal() -> Option<Vec<String>> {
    let own = std::env::var("TERMINAL").ok();
    terminal_in(&env_path(), own.as_deref(), |launcher| first_line(&[launcher.to_str().unwrap_or(""), "--print-id"], Duration::from_secs(3)).is_some())
}

/// Starts `argv` on its own (own process group, in the home folder), not waited for.
pub fn spawn_detached(argv: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let (program, args) = argv.split_first().ok_or_else(|| std::io::Error::other("nothing to run"))?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .current_dir(crate::paths::home())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Why a terminal could not be opened (shown or logged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NoTerminal,
    NoHelper,
    Failed(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NoTerminal => f.write_str("no terminal found"),
            OpenError::NoHelper => f.write_str("no AI helper installed"),
            OpenError::Failed(e) => f.write_str(e),
        }
    }
}

/// "Details": `coredumpctl info <pid>` in a terminal.
pub fn open_details(crash: &Crash) -> Result<(), OpenError> {
    let terminal = terminal().ok_or(OpenError::NoTerminal)?;
    spawn_detached(&details_argv(&terminal, crash.pid)).map_err(|e| OpenError::Failed(e.to_string()))
}

/// "Check with AI": the chosen helper (the shared choice, [crate::shared_ai]) in a terminal with the
/// crash prompt; one that isn't installed falls back to the first installed one.
pub fn open_ai(crash: &Crash, setting: &str) -> Result<(), OpenError> {
    let installed = crate::helpers::installed();
    let helper = crate::helpers::choose(setting, &installed).ok_or(OpenError::NoHelper)?;
    let terminal = terminal().ok_or(OpenError::NoTerminal)?;
    let argv: Vec<String> = terminal.into_iter().chain(crate::helpers::task_argv(helper, &prompt(crash))).collect();
    spawn_detached(&argv).map_err(|e| OpenError::Failed(e.to_string()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const LINE: &str = r#"{"MESSAGE_ID":"fc2e22bc6ee647b6b90729ab34a250b1","COREDUMP_EXE":"/usr/bin/cat","COREDUMP_COMM":"cat","COREDUMP_SIGNAL_NAME":"SIGSEGV","COREDUMP_SIGNAL":"11","COREDUMP_UID":"1000","COREDUMP_PID":"4242","__REALTIME_TIMESTAMP":"1794557523123456"}"#;

    #[test]
    fn journal_lines() {
        let crash = parse_line(LINE, 1000, 5).unwrap();
        assert_eq!(crash, Crash { pid: 4242, exe: "/usr/bin/cat".into(), name: "cat".into(), signal: "SIGSEGV".into(), time: 1_794_557_523 });
        // Another user's crash, garbage, not a coredump.
        assert_eq!(parse_line(LINE, 1001, 5), None);
        for bad in ["", "garbage", "{", "[1,2]", "null", r#"{"MESSAGE_ID":"other","COREDUMP_UID":"1000","COREDUMP_PID":"1","COREDUMP_EXE":"/x"}"#] {
            assert_eq!(parse_line(bad, 1000, 5), None, "{bad}");
        }
        // Missing fields: no uid or pid → nothing; no exe and no comm → nothing.
        assert_eq!(parse_line(r#"{"COREDUMP_PID":"1","COREDUMP_EXE":"/x"}"#, 1000, 5), None);
        assert_eq!(parse_line(r#"{"COREDUMP_UID":"1000","COREDUMP_EXE":"/x"}"#, 1000, 5), None);
        assert_eq!(parse_line(r#"{"COREDUMP_UID":"1000","COREDUMP_PID":"0","COREDUMP_EXE":"/x"}"#, 1000, 5), None);
        assert_eq!(parse_line(r#"{"COREDUMP_UID":"1000","COREDUMP_PID":"3"}"#, 1000, 5), None);
        // Defaults: name from the exe, signal from its number, time now.
        let c = parse_line(r#"{"COREDUMP_UID":"1000","COREDUMP_PID":"3","COREDUMP_EXE":"/opt/x/app","COREDUMP_SIGNAL":"6"}"#, 1000, 77).unwrap();
        assert_eq!((c.name.as_str(), c.signal.as_str(), c.time), ("app", "signal 6", 77));
        let c = parse_line(r#"{"COREDUMP_UID":"1000","COREDUMP_PID":"3","COREDUMP_COMM":"x"}"#, 1000, 77).unwrap();
        assert_eq!((c.exe.as_str(), c.signal.as_str()), ("", "?"));
        // Bytes (non-UTF-8 values) and several values; control characters go.
        let c = parse_line(r#"{"COREDUMP_UID":["1000","1000"],"COREDUMP_PID":"3","COREDUMP_COMM":[98,97,100,10,255]}"#, 1000, 0).unwrap();
        assert_eq!(c.name, "bad \u{fffd}");
    }

    #[test]
    fn bursts_and_rate_limit() {
        let mut c = Coalescer::default();
        assert_eq!(c.decide("/a", 0), Decision::New);
        c.sent("/a", 10);
        assert_eq!(c.decide("/a", 1_000), Decision::Update { replaces_id: 10, count: 2 });
        assert_eq!(c.decide("/a", 2_000), Decision::Update { replaces_id: 10, count: 3 });
        assert_eq!(c.decide("/b", 3_000), Decision::New);
        assert_eq!(c.decide("/c", 4_000), Decision::New);
        // Five sent within a minute: then nothing.
        assert_eq!(c.decide("/d", 5_000), Decision::Drop);
        // 60 s after its last crash it is new again (and the old sends left the window).
        assert_eq!(c.decide("/a", 62_000), Decision::New);
        c.sent("/a", 11);
        assert_eq!(c.decide("/a", 63_000), Decision::Update { replaces_id: 11, count: 2 });
        let mut c = Coalescer::default();
        for (i, exe) in ["/1", "/2", "/3", "/4", "/5"].iter().enumerate() {
            assert_eq!(c.decide(exe, i as u64), Decision::New);
            c.sent(exe, i as u32 + 1);
        }
        assert_eq!(c.decide("/6", 10), Decision::Drop);
        // A dropped repeat still counts for the next update.
        assert_eq!(c.decide("/1", 20), Decision::Drop);
        assert_eq!(c.decide("/1", 60_000), Decision::Update { replaces_id: 1, count: 3 });
        assert_eq!(c.decide("/6", 60_001), Decision::New);
    }

    #[test]
    fn notification_arguments() {
        let crash = Crash { pid: 7, exe: "/opt/a<b>&c".into(), name: "A".into(), signal: "SIGABRT".into(), time: 0 };
        let n = notify_args(&crash, 1, 0, false);
        assert_eq!((n.app_name.as_str(), n.app_icon.as_str(), n.replaces_id, n.timeout), (APP, ICON, 0, -1));
        assert_eq!(n.summary, "A crashed");
        assert!(n.body.starts_with("SIGABRT\n/opt/a&lt;b&gt;&amp;c\n"), "{}", n.body);
        assert_eq!(n.actions, vec!["details", "Details"]);
        assert_eq!(n.hints["urgency"], zbus::zvariant::Value::U8(1));
        assert_eq!(n.hints["category"], zbus::zvariant::Value::from("crash"));
        assert_eq!(n.hints[HINT_PID], zbus::zvariant::Value::U32(7));
        assert!(!n.hints.contains_key("transient"));
        let n = notify_args(&crash, 3, 42, true);
        assert_eq!((n.summary.as_str(), n.replaces_id), ("A crashed 3 times", 42));
        assert_eq!(n.actions, vec!["details", "Details", "ai", "Check with AI"]);
    }

    pub fn program(dir: &Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub fn scratch(name: &str) -> PathBuf {
        let dir = crate::paths::test_roots().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn argv_quoting() {
        let odd = Crash { pid: 9, exe: "/tmp/$(rm -rf ~); `x` \"q\" 'y'".into(), name: "-rf".into(), signal: "SIGSEGV".into(), time: 0 };
        let text = prompt(&odd);
        assert!(text.starts_with("-rf (/tmp/$(rm -rf ~); `x` \"q\" 'y') crashed with SIGSEGV at "), "{text}");
        assert!(text.contains("PID 9. Read `coredumpctl info 9`"));
        assert_eq!(details_argv(&["/usr/bin/konsole".into(), "-e".into()], 9), vec!["/usr/bin/konsole", "-e", "sh", "-c", "coredumpctl info \"$1\" 2>&1 | less", "sh", "9"]);
    }

    #[test]
    fn terminal_choice() {
        let dir = scratch("terminals");
        let bin = dir.join("bin");
        let path = bin.to_str().unwrap();
        assert_eq!(terminal_in(path, None, |_| true), None);
        program(&bin, "xterm", "");
        program(&bin, "foot", "");
        assert_eq!(terminal_in(path, None, |_| true).unwrap(), vec![bin.join("foot").to_string_lossy().into_owned()]);
        program(&bin, "konsole", "");
        assert_eq!(terminal_in(path, Some("xterm"), |_| true).unwrap()[1..], ["-e"]);
        // Not a plain name, or not installed: the list.
        assert_eq!(terminal_in(path, Some("xterm -x"), |_| true).unwrap()[1..], ["-e"]);
        assert_eq!(terminal_in(path, Some("xterm -x"), |_| true).unwrap()[0], bin.join("konsole").to_string_lossy());
        program(&bin, "myterm", "");
        assert_eq!(terminal_in(path, Some("myterm"), |_| true).unwrap()[1..], ["-e"]);
        // xdg-terminal-exec first, when it resolves a terminal.
        program(&bin, "xdg-terminal-exec", "");
        assert_eq!(terminal_in(path, Some("myterm"), |_| true).unwrap(), vec![bin.join("xdg-terminal-exec").to_string_lossy().into_owned()]);
        assert_eq!(terminal_in(path, Some("myterm"), |_| false).unwrap()[0], bin.join("myterm").to_string_lossy());
    }

    #[test]
    fn names_from_desktop_entries() {
        let dir = scratch("applications");
        std::fs::write(dir.join("b.desktop"), "[Desktop Entry]\nName=Firefox\nExec=/usr/lib/firefox/firefox %u\n[Desktop Action new]\nName=New\nExec=other\n").unwrap();
        std::fs::write(dir.join("a.desktop"), "[Desktop Entry]\nName=Shell thing\nExec=sh -c x\n").unwrap();
        std::fs::write(dir.join("c.desktop"), "[Desktop Entry]\nName=Hidden one\nExec=hidden\nHidden=true\n").unwrap();
        std::fs::write(dir.join("d.desktop"), "[Desktop Entry]\nName=Cat\nExec=\"cat\" %f\n").unwrap();
        let dirs = vec![dir.join("missing"), dir.clone()];
        assert_eq!(desktop_name("/usr/lib/firefox/firefox", &dirs).as_deref(), Some("Firefox"));
        assert_eq!(desktop_name("/usr/bin/cat", &dirs).as_deref(), Some("Cat"));
        assert_eq!(desktop_name("/usr/bin/sh", &dirs), None);
        assert_eq!(desktop_name("/usr/bin/hidden", &dirs), None);
        assert_eq!(desktop_name("/usr/bin/other", &dirs), None);
        assert_eq!(desktop_name("", &dirs), None);
    }
}
