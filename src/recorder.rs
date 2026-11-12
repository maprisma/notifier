//! The recorder: watches the session bus read-only (`BecomeMonitor`), writes what it sees to the
//! history and applies do not disturb on this desktop. One per session (the D-Bus name
//! [NAME], DoNotQueue). It never answers, changes or sends a notification itself.
//!
//! The tray (KDE, Hyprland) and the applet (COSMIC) start it with [supervise]; it ends with them
//! (PR_SET_PDEATHSIG). Started by hand it runs until the bus goes away.

use crate::notify::{self, Event};
use crate::settings::{self, Settings};
use futures_util::StreamExt;
use std::collections::HashMap;
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

/// Applies do not disturb where this desktop reads it. `kde` holds Plasma's inhibition.
async fn apply(target: Option<crate::dnd::Target>, on: bool, connection: &zbus::Connection, kde: &mut crate::dnd::Kde) {
    use crate::dnd::Target;
    let result: Result<(), String> = match target {
        Some(Target::Shell) => crate::dnd::set_shell(on).map(|_| ()).map_err(|e| e.to_string()),
        Some(Target::Cosmic) => crate::dnd::set_cosmic(on).map(|_| ()).map_err(|e| e.to_string()),
        Some(Target::Kde) => kde.set(connection, on).await.map_err(|e| e.to_string()),
        None => Ok(()),
    };
    if let Err(error) = result {
        eprintln!("maprisma-notifier-recorder: do not disturb: {error}");
    }
}

/// Runs the recorder; returns the process exit code.
pub async fn run(target: Option<crate::dnd::Target>) -> i32 {
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
    let mut applied = settings::effective_dnd(&current, settings::local_minutes());
    if applied && target == Some(crate::dnd::Target::Kde) {
        apply(target, true, &connection, &mut kde).await;
    }

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
                    Some(Event::Action { id }) => { let _ = crate::history::mark_closed(id, true); }
                    None => {}
                }
            }
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
                if effective != applied {
                    applied = effective;
                    apply(target, applied, &connection, &mut kde).await;
                }
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
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl FakeServer {
        #[allow(clippy::too_many_arguments)]
        fn notify(&self, _app: String, replaces: u32, _icon: String, _summary: String, _body: String, _actions: Vec<String>, _hints: HashMap<String, zbus::zvariant::OwnedValue>, _timeout: i32) -> u32 {
            if replaces != 0 { replaces } else { self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) }
        }
        fn inhibit(&self, _entry: String, _reason: String, _hints: HashMap<String, zbus::zvariant::OwnedValue>) -> u32 {
            self.inhibited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            7
        }
        fn un_inhibit(&self, cookie: u32) {
            assert_eq!(cookie, 7);
            self.inhibited.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
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
        let server = zbus::connection::Builder::session()
            .unwrap()
            .name(notify::INTERFACE)
            .unwrap()
            .serve_at(notify::PATH, FakeServer { next: 100.into(), inhibited: inhibited.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();
        let recorder = tokio::spawn(run(Some(crate::dnd::Target::Kde)));
        // One recorder per session.
        let client = zbus::Connection::session().await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        for _ in 0..100 {
            if dbus.name_has_owner(NAME.try_into().unwrap()).await.unwrap() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), run(None)).await.unwrap(), EXIT_TAKEN);
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

        // dnd off: released within a tick.
        settings::set("dnd", Some(false.into())).unwrap();
        let held = inhibited.clone();
        wait_for("uninhibit", || held.load(std::sync::atomic::Ordering::SeqCst) == 0).await;

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
        let recorder = tokio::spawn(run(Some(crate::dnd::Target::Shell)));
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
}
