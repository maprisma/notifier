//! The recorder: watches the session bus read-only (`BecomeMonitor`), writes what it sees to the
//! history and applies do not disturb on this desktop. One per session (the D-Bus name
//! [NAME], DoNotQueue). It never answers or changes another app's notification; the only ones it
//! sends are its own crash reports (crash.rs): it follows systemd-coredump's journal entries
//! (`journalctl -f`, a child that ends with it) and opens Details / Check with AI when their
//! actions are invoked. Its own notifications reach the history like any other, through the
//! monitor.
//!
//! The tray (KDE, Hyprland) and the applet (COSMIC) start it with [supervise]; it ends with them
//! (PR_SET_PDEATHSIG). Started by hand it runs until the bus goes away.

use crate::notify::{self, Event};
use crate::settings::{self, Settings};
use futures_util::StreamExt;
use crate::crash::{self, Crash};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime};

pub const NAME: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier.Recorder";
/// Exit code when another recorder runs already.
pub const EXIT_TAKEN: i32 = 3;
/// A Notify call without a reply after this is written without the server's id.
const REPLY_WAIT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_secs(2);

/// What a Notify call needs until its reply comes.
struct Pending {
    notify: notify::Notify,
    at: Instant,
    time: i64,
}

/// The message filters for the monitor (Notify calls, the server's replies, its signals).
pub fn match_rules() -> Vec<String> {
    let i = notify::INTERFACE;
    vec![
        format!("type='method_call',interface='{i}',member='Notify'"),
        format!("type='method_return',sender='{i}'"),
        format!("type='signal',interface='{i}',member='NotificationClosed'"),
        format!("type='signal',interface='{i}',member='ActionInvoked'"),
    ]
}

/// Whether a notification goes into the history at all.
pub fn wanted(n: &notify::Notify, settings: &Settings) -> bool {
    !n.transient && !settings.hidden_apps.contains(&notify::app_label(n))
}

/// The state of the files that matter (settings), to notice changes cheaply.
fn stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn record(n: &notify::Notify, server_id: u32, time: i64, settings: &Settings) {
    let entry = notify::to_entry(n, server_id, time);
    let result = if n.replaces_id != 0 {
        match crate::history::replace(n.replaces_id, &entry.app, entry.clone()) {
            Ok(true) => Ok(()),
            _ => crate::history::append(entry, settings),
        }
    } else {
        crate::history::append(entry, settings)
    };
    if let Err(error) = result {
        eprintln!("maprisma-notifier-recorder: history: {error}");
    }
}

/// Applies do not disturb where this desktop reads it. `kde` holds Plasma's inhibition. Whether
/// it worked: the caller counts a state as applied only then, and tries again on the next tick.
async fn apply(target: Option<crate::dnd::Target>, on: bool, connection: &zbus::Connection, kde: &mut crate::dnd::Kde) -> Result<(), String> {
    use crate::dnd::Target;
    match target {
        Some(Target::Shell) => crate::dnd::set_shell(on).map(|_| ()).map_err(|e| e.to_string()),
        Some(Target::Cosmic) => crate::dnd::set_cosmic(on).map(|_| ()).map_err(|e| e.to_string()),
        Some(Target::Kde) => kde.set(connection, on).await.map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// The do not disturb state the recorder last put on the desktop, retried until it works.
struct Applied {
    state: bool,
    /// The last attempt failed (logged once per failing run, not every tick).
    failing: bool,
}

impl Applied {
    /// Brings the desktop to `effective` when it isn't there yet (a change, or a failed attempt).
    async fn sync(&mut self, effective: bool, target: Option<crate::dnd::Target>, connection: &zbus::Connection, kde: &mut crate::dnd::Kde) {
        if effective == self.state {
            self.failing = false;
            return;
        }
        match apply(target, effective, connection, kde).await {
            Ok(()) => {
                self.state = effective;
                self.failing = false;
            }
            Err(error) => {
                if !self.failing {
                    eprintln!("maprisma-notifier-recorder: do not disturb: {error} (trying again)");
                }
                self.failing = true;
            }
        }
    }
}

/// `journalctl` following systemd-coredump's entries from now on.
pub fn journal_argv() -> Vec<String> {
    ["journalctl", "-f", "-o", "json", "-n", "0", "--no-pager"].into_iter().map(String::from).chain([format!("MESSAGE_ID={}", crash::MESSAGE_ID)]).collect()
}

/// Aborts a task when dropped (the journal follower ends with the recorder).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs `argv` (journalctl) and sends the user's crashes it prints; started again when it ends
/// (5 s, doubling up to 5 min; back to 5 s after a long run). Missing journalctl: crash reports
/// are off (logged once). The child is killed with this task and gets SIGTERM when the recorder
/// dies.
async fn follow_journal(argv: Vec<String>, sender: tokio::sync::mpsc::Sender<Crash>) {
    use tokio::io::AsyncBufReadExt;
    let Some((program, args)) = argv.split_first() else { return };
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let mut delay = Duration::from_secs(5);
    loop {
        let mut command = tokio::process::Command::new(program);
        command.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let started = Instant::now();
        match command.spawn() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("maprisma-notifier-recorder: {program} not found: crash reports are off");
                return;
            }
            Err(error) => eprintln!("maprisma-notifier-recorder: {program}: {error}"),
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    let mut lines = tokio::io::BufReader::new(out).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(found) = crash::parse_line(&line, uid, settings::now_unix())
                            && sender.send(found).await.is_err()
                        {
                            return;
                        }
                    }
                }
                let _ = child.wait().await;
            }
        }
        if started.elapsed() > Duration::from_secs(300) {
            delay = Duration::from_secs(5);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(300));
    }
}

/// The recorder's own crash notifications, by the server's id (for their actions).
#[derive(Default)]
struct Reports {
    coalescer: crash::Coalescer,
    by_id: HashMap<u32, Crash>,
    order: VecDeque<u32>,
    started: Option<Instant>,
}

impl Reports {
    fn remember(&mut self, id: u32, crash: Crash) {
        if id == 0 {
            return;
        }
        if self.by_id.insert(id, crash).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > 100 {
            if let Some(old) = self.order.pop_front() {
                self.by_id.remove(&old);
            }
        }
    }

    /// One crash: a notification (or an update of the program's last one), unless crash reports
    /// are off or too many came in the last minute.
    async fn report(&mut self, mut crash: Crash, settings: &Settings, connection: &zbus::Connection) {
        if !settings.notify_crashes {
            return;
        }
        let key = if crash.exe.is_empty() { crash.name.clone() } else { crash.exe.clone() };
        let now = self.started.get_or_insert_with(Instant::now).elapsed().as_millis() as u64;
        let (count, replaces_id) = match self.coalescer.decide(&key, now) {
            crash::Decision::Drop => return,
            crash::Decision::New => (1, 0),
            crash::Decision::Update { replaces_id, count } => (count, replaces_id),
        };
        let exe = crash.exe.clone();
        let (name, ai) = tokio::task::spawn_blocking(move || (crash::desktop_name(&exe, &crash::application_dirs()), !crate::helpers::installed().is_empty()))
            .await
            .unwrap_or((None, false));
        if let Some(name) = name {
            crash.name = name;
        }
        let a = crash::notify_args(&crash, count, replaces_id, ai);
        let body = (a.app_name.as_str(), a.replaces_id, a.app_icon.as_str(), a.summary.as_str(), a.body.as_str(), &a.actions, &a.hints, a.timeout);
        let call = connection.call_method(Some(notify::INTERFACE), notify::PATH, Some(notify::INTERFACE), "Notify", &body);
        let id = match tokio::time::timeout(Duration::from_secs(5), call).await {
            Ok(Ok(reply)) => reply.body().deserialize::<u32>().unwrap_or(0),
            Ok(Err(error)) => {
                eprintln!("maprisma-notifier-recorder: crash notification: {error}");
                0
            }
            Err(_) => {
                eprintln!("maprisma-notifier-recorder: crash notification: no answer");
                0
            }
        };
        self.coalescer.sent(&key, id);
        self.remember(id, crash);
    }

    /// Details / Check with AI on one of ours.
    fn action(&self, id: u32, key: &str, settings: &Settings) {
        let Some(crash) = self.by_id.get(&id).cloned() else { return };
        // The helper all Maprisma apps use (Maprisma Manager → AI), read now; the old own
        // setting only while that file doesn't exist.
        let old = settings.ai_helper.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            let result = match key.as_str() {
                crash::ACTION_DETAILS => crash::open_details(&crash),
                crash::ACTION_AI => crash::open_ai(&crash, &crate::shared_ai::choice(&old)),
                _ => return,
            };
            if let Err(error) = result {
                eprintln!("maprisma-notifier-recorder: {key}: {error}");
            }
        });
    }
}

/// Runs the recorder with crash reports; returns the process exit code.
pub async fn run(target: Option<crate::dnd::Target>) -> i32 {
    run_with(target, Some(journal_argv())).await
}

/// The recorder; `journal`: the command printing coredump entries (None: no crash reports).
pub async fn run_with(target: Option<crate::dnd::Target>, journal: Option<Vec<String>>) -> i32 {
    let connection = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(error) => {
            eprintln!("maprisma-notifier-recorder: session bus: {error}");
            return 1;
        }
    };
    match connection.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into()).await {
        Ok(_) => {}
        Err(zbus::Error::NameTaken) => return EXIT_TAKEN,
        Err(error) => {
            eprintln!("maprisma-notifier-recorder: {NAME}: {error}");
            return 1;
        }
    }
    // A second connection only listens: after BecomeMonitor it can't send anything.
    let monitor = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(error) => {
            eprintln!("maprisma-notifier-recorder: session bus: {error}");
            return 1;
        }
    };
    let mut stream = zbus::MessageStream::from(&monitor);
    let rules = match_rules();
    let rules: Vec<&str> = rules.iter().map(String::as_str).collect();
    if let Err(error) = monitor
        .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus.Monitoring"), "BecomeMonitor", &(rules, 0u32))
        .await
    {
        eprintln!("maprisma-notifier-recorder: BecomeMonitor: {error}");
        return 1;
    }

    let settings_path = crate::paths::settings_file();
    let mut settings_stamp = stamp(&settings_path);
    let mut current = settings::load();
    let mut kde = crate::dnd::Kde::default();
    // Written only when the effective state changes (the switch, a schedule boundary): a change
    // made on the desktop itself or by Maprisma Manager is not fought. Only Plasma's
    // inhibition, which lives with this connection, is taken at once.
    let effective = settings::effective_dnd(&current, settings::local_minutes());
    let kde_target = target == Some(crate::dnd::Target::Kde);
    // On KDE nothing is held yet: start from "off" so an "on" is taken (and retried) now.
    let mut applied = Applied { state: if kde_target { false } else { effective }, failing: false };
    applied.sync(effective, target, &connection, &mut kde).await;

    let (crash_sender, mut crashes) = tokio::sync::mpsc::channel::<Crash>(32);
    let _follower = journal.map(|argv| AbortOnDrop(tokio::spawn(follow_journal(argv, crash_sender.clone()))));
    let mut reports = Reports::default();

    let mut pending: HashMap<(String, u32), Pending> = HashMap::new();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            message = stream.next() => {
                let Some(Ok(message)) = message else { break };
                match notify::event(&message) {
                    Some(Event::Notify { sender, serial, notify }) => {
                        if wanted(&notify, &current) {
                            pending.insert((sender, serial), Pending { notify, at: Instant::now(), time: settings::now_unix() });
                        }
                    }
                    Some(Event::Reply { destination, reply_serial, server_id }) => {
                        if let Some(p) = pending.remove(&(destination, reply_serial)) {
                            record(&p.notify, server_id.unwrap_or(0), p.time, &current);
                        }
                    }
                    Some(Event::Closed { id, .. }) => { let _ = crate::history::mark_closed(id, false); }
                    // The user clicked it: seen.
                    Some(Event::Action { id, key }) => {
                        let _ = crate::history::mark_closed(id, true);
                        reports.action(id, &key, &current);
                    }
                    None => {}
                }
            }
            // crash_sender is held here, so this never ends.
            Some(found) = crashes.recv() => reports.report(found, &current, &connection).await,
            _ = tick.tick() => {
                let stale: Vec<(String, u32)> = pending.iter().filter(|(_, p)| p.at.elapsed() >= REPLY_WAIT).map(|(k, _)| k.clone()).collect();
                for key in stale {
                    if let Some(p) = pending.remove(&key) {
                        record(&p.notify, 0, p.time, &current);
                    }
                }
                let now_stamp = stamp(&settings_path);
                if now_stamp != settings_stamp {
                    settings_stamp = now_stamp;
                    current = settings::load();
                }
                let effective = settings::effective_dnd(&current, settings::local_minutes());
                applied.sync(effective, target, &connection, &mut kde).await;
            }
        }
    }
    0
}

/// Keeps a recorder running beside the calling process (tray, applet): starts
/// `maprisma-notifier-recorder` next to this executable, again after it ends (30 s when another
/// recorder holds the name, else 5 s). The child gets SIGTERM when this process ends.
pub fn supervise() {
    let Ok(exe) = std::env::current_exe() else { return };
    let recorder = exe.with_file_name("maprisma-notifier-recorder");
    std::thread::spawn(move || {
        loop {
            let delay = match spawn_child(&recorder) {
                Ok(mut child) => match child.wait() {
                    Ok(status) if status.code() == Some(EXIT_TAKEN) => Duration::from_secs(30),
                    _ => Duration::from_secs(5),
                },
                Err(_) => Duration::from_secs(30),
            };
            std::thread::sleep(delay);
        }
    });
}

fn spawn_child(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    let mut command = std::process::Command::new(path);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent ended before prctl: don't run without it.
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_filter() {
        let rules = match_rules();
        assert_eq!(rules.len(), 4);
        assert!(rules[0].contains("member='Notify'"));
        let mut settings = Settings::default();
        let n = notify::Notify { app_name: "Spotify".into(), ..Default::default() };
        assert!(wanted(&n, &settings));
        settings.hidden_apps = vec!["Spotify".into()];
        assert!(!wanted(&n, &settings));
        let t = notify::Notify { app_name: "Volume".into(), transient: true, ..Default::default() };
        assert!(!wanted(&t, &Settings::default()));
    }

    /// A stand-in notification server: answers Notify with ids from 100 and counts inhibitions.
    struct FakeServer {
        next: std::sync::atomic::AtomicU32,
        inhibited: std::sync::Arc<std::sync::atomic::AtomicI32>,
        /// UnInhibit calls that fail (counted down), then they work.
        fail_uninhibit: std::sync::Arc<std::sync::atomic::AtomicI32>,
        /// Every UnInhibit call, failed or not.
        uninhibit_calls: std::sync::Arc<std::sync::atomic::AtomicI32>,
        /// (app, replaces, summary, actions) of every Notify.
        calls: std::sync::Arc<std::sync::Mutex<Vec<(String, u32, String, Vec<String>)>>>,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl FakeServer {
        #[allow(clippy::too_many_arguments)]
        fn notify(&self, app: String, replaces: u32, _icon: String, summary: String, _body: String, actions: Vec<String>, _hints: HashMap<String, zbus::zvariant::OwnedValue>, _timeout: i32) -> u32 {
            self.calls.lock().unwrap().push((app, replaces, summary, actions));
            if replaces != 0 { replaces } else { self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) }
        }
        fn inhibit(&self, _entry: String, _reason: String, _hints: HashMap<String, zbus::zvariant::OwnedValue>) -> u32 {
            self.inhibited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            7
        }
        fn un_inhibit(&self, cookie: u32) -> zbus::fdo::Result<()> {
            use std::sync::atomic::Ordering::SeqCst;
            assert_eq!(cookie, 7);
            self.uninhibit_calls.fetch_add(1, SeqCst);
            if self.fail_uninhibit.fetch_sub(1, SeqCst) > 0 {
                return Err(zbus::fdo::Error::Failed("busy".into()));
            }
            self.inhibited.fetch_sub(1, SeqCst);
            Ok(())
        }
        #[zbus(signal)]
        async fn notification_closed(emitter: &zbus::object_server::SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;
    }

    async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..100 {
            if ok() { return; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out: {what}");
    }

    async fn send(client: &zbus::Connection, app: &str, replaces: u32, summary: &str, hints: HashMap<&str, zbus::zvariant::Value<'_>>) -> u32 {
        let reply = client
            .call_method(Some(notify::INTERFACE), notify::PATH, Some(notify::INTERFACE), "Notify", &(app, replaces, "icon-name", summary, "<b>Body</b> &amp; more", Vec::<&str>::new(), hints, 5000i32))
            .await
            .unwrap();
        reply.body().deserialize::<u32>().unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "run under dbus-run-session with --ignored"]
    async fn records_from_the_bus_and_inhibits() {
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let _ = std::fs::remove_file(crate::paths::history_file());
        settings::set("hidden_apps", Some(serde_json::json!(["Hidden"]))).unwrap();
        settings::set("dnd", Some(true.into())).unwrap();
        settings::set("dnd_schedule", None).unwrap();

        let inhibited = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let fail_uninhibit = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let uninhibit_calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let server = zbus::connection::Builder::session()
            .unwrap()
            .name(notify::INTERFACE)
            .unwrap()
            .serve_at(notify::PATH, FakeServer { next: 100.into(), inhibited: inhibited.clone(), fail_uninhibit: fail_uninhibit.clone(), uninhibit_calls: uninhibit_calls.clone(), calls: Default::default() })
            .unwrap()
            .build()
            .await
            .unwrap();
        let recorder = tokio::spawn(run_with(Some(crate::dnd::Target::Kde), None));
        // One recorder per session.
        let client = zbus::Connection::session().await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        for _ in 0..100 {
            if dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), run_with(None, None)).await.unwrap(), EXIT_TAKEN);
        // dnd on: Plasma's inhibition is held.
        let held = inhibited.clone();
        wait_for("inhibit", || held.load(std::sync::atomic::Ordering::SeqCst) == 1).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut hints = HashMap::new();
        hints.insert("urgency", zbus::zvariant::Value::U8(2));
        let first = send(&client, "Mail", 0, "Hello", hints).await;
        assert_eq!(first, 100);
        send(&client, "Hidden", 0, "secret", HashMap::new()).await;
        let mut transient = HashMap::new();
        transient.insert("transient", zbus::zvariant::Value::Bool(true));
        send(&client, "Volume", 0, "50 %", transient).await;
        let second = send(&client, "Chat", 0, "Hi", HashMap::new()).await;
        wait_for("two entries", || crate::history::load().len() == 2).await;
        let list = crate::history::load();
        assert_eq!((list[0].app.as_str(), list[0].summary.as_str(), list[0].body.as_str()), ("Mail", "Hello", "Body & more"));
        assert_eq!((list[0].server_id, list[0].urgency, list[0].read), (100, 2, false));
        assert_eq!((list[1].app.as_str(), list[1].server_id), ("Chat", second));

        // replaces_id updates the entry in place.
        send(&client, "Chat", second, "Hi again", HashMap::new()).await;
        wait_for("replaced", || crate::history::load().get(1).is_some_and(|e| e.summary == "Hi again")).await;
        assert_eq!(crate::history::load().len(), 2);

        // The server closes one.
        let emitter = zbus::object_server::SignalEmitter::new(&server, notify::PATH).unwrap();
        FakeServer::notification_closed(&emitter, 100, 2).await.unwrap();
        wait_for("closed", || crate::history::load()[0].closed).await;

        // dnd off: the first release fails; the cookie is kept and the release tried again.
        fail_uninhibit.store(1, std::sync::atomic::Ordering::SeqCst);
        settings::set("dnd", Some(false.into())).unwrap();
        let held = inhibited.clone();
        wait_for("uninhibit", || held.load(std::sync::atomic::Ordering::SeqCst) == 0).await;
        assert_eq!(uninhibit_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        // Released for good: no more attempts.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(uninhibit_calls.load(std::sync::atomic::Ordering::SeqCst), 2);

        recorder.abort();
        settings::set("hidden_apps", None).unwrap();
        settings::set("dnd", None).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "run under dbus-run-session with --ignored"]
    async fn shell_written_only_on_changes() {
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let client = zbus::Connection::session().await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        // The other test's recorder may still be going away.
        for _ in 0..100 {
            if !dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let shell = crate::paths::shell_file();
        std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
        std::fs::write(&shell, r#"{"notifications": {"dnd": false, "timeout": 8}}"#).unwrap();
        settings::set("dnd_schedule", None).unwrap();
        settings::set("dnd", Some(true.into())).unwrap();
        let read = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(crate::paths::shell_file()).unwrap()).unwrap();
        let recorder = tokio::spawn(run_with(Some(crate::dnd::Target::Shell), None));
        // At start nothing is written: the desktop's own (manual) state stays.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(read()["notifications"]["dnd"], false);
        // The effective state changes: off (already off), then on again.
        settings::set("dnd", Some(false.into())).unwrap();
        tokio::time::sleep(Duration::from_millis(2500)).await;
        settings::set("dnd", Some(true.into())).unwrap();
        wait_for("shell dnd", || read()["notifications"]["dnd"] == true).await;
        assert_eq!(read()["notifications"]["timeout"], 8);
        recorder.abort();
        settings::set("dnd", None).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "run under dbus-run-session with --ignored"]
    async fn a_failed_dnd_write_is_tried_again() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let client = zbus::Connection::session().await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        for _ in 0..100 {
            if !dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let shell = crate::paths::shell_file();
        let folder = shell.parent().unwrap().to_owned();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&shell, r#"{"notifications": {"dnd": false}}"#).unwrap();
        settings::set("dnd_schedule", None).unwrap();
        settings::set("dnd", Some(false.into())).unwrap();
        let read = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(crate::paths::shell_file()).unwrap()).unwrap();
        let recorder = tokio::spawn(run_with(Some(crate::dnd::Target::Shell), None));
        tokio::time::sleep(Duration::from_millis(500)).await;
        // The Shell's folder can't be written: the change fails...
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        settings::set("dnd", Some(true.into())).unwrap();
        tokio::time::sleep(Duration::from_millis(4500)).await;
        assert_eq!(read()["notifications"]["dnd"], false);
        // ...and is made once it can be.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        wait_for("shell dnd after the retry", || read()["notifications"]["dnd"] == true).await;
        recorder.abort();
        settings::set("dnd", None).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "run under dbus-run-session with --ignored"]
    async fn a_crash_line_becomes_one_notification_and_one_entry() {
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::test_roots();
        let _ = std::fs::remove_file(crate::paths::history_file());
        for key in ["hidden_apps", "dnd", "dnd_schedule", "notify_crashes"] {
            settings::set(key, None).unwrap();
        }
        let client = zbus::Connection::session().await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        for _ in 0..100 {
            if !dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let calls = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let _server = zbus::connection::Builder::session()
            .unwrap()
            .name(notify::INTERFACE)
            .unwrap()
            .serve_at(notify::PATH, FakeServer { next: 200.into(), inhibited: Default::default(), fail_uninhibit: Default::default(), uninhibit_calls: Default::default(), calls: calls.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let line = |pid: u32, who: u32| format!(r#"{{"MESSAGE_ID":"{}","COREDUMP_EXE":"/usr/bin/notifier-test-crasher","COREDUMP_COMM":"crasher","COREDUMP_SIGNAL_NAME":"SIGSEGV","COREDUMP_UID":"{who}","COREDUMP_PID":"{pid}","__REALTIME_TIMESTAMP":"1794557523000000"}}"#, crash::MESSAGE_ID);
        // Garbage, another user's crash, ours; then (a second later) ours again.
        let script = format!("echo garbage; echo '{}'; echo '{}'; sleep 1; echo '{}'; exec sleep 60", line(5, uid + 1), line(6, uid), line(7, uid));
        let recorder = tokio::spawn(run_with(None, Some(vec!["sh".into(), "-c".into(), script])));

        wait_for("one entry", || crate::history::load().len() == 1).await;
        let list = crate::history::load();
        let entry = &list[0];
        assert_eq!((entry.app.as_str(), entry.icon.as_str(), entry.server_id), ("Crashes", "dialog-error", 200));
        assert_eq!(entry.summary, "crasher crashed");
        assert_eq!(entry.crash.as_ref().map(|c| c.pid), Some(6));
        // The second crash updates the same notification and entry.
        wait_for("updated", || crate::history::load().first().is_some_and(|e| e.summary == "crasher crashed 2 times")).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let list = crate::history::load();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].crash.as_ref().map(|c| c.pid), Some(7));
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!((calls[0].0.as_str(), calls[0].1), ("Crashes", 0));
        assert_eq!(calls[1].1, 200);
        assert_eq!(&calls[0].3[..2], ["details", "Details"]);

        // Turned off: nothing more is sent.
        recorder.abort();
        settings::set("notify_crashes", Some(false.into())).unwrap();
        for _ in 0..100 {
            if !dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let script = format!("echo '{}'; exec sleep 60", line(8, uid));
        let recorder = tokio::spawn(run_with(None, Some(vec!["sh".into(), "-c".into(), script])));
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(crate::history::load().len(), 1);
        recorder.abort();
        settings::set("notify_crashes", None).unwrap();
    }
}
