//! What the recorder reads off the session bus: `org.freedesktop.Notifications.Notify` calls
//! (`susssasa{sv}i`), their replies (the server's id) and the `NotificationClosed` /
//! `ActionInvoked` signals. Pure functions of a message, tested below without a bus.

use std::collections::HashMap;
use zbus::message::{Message, Type};
use zbus::zvariant::{OwnedValue, Value};

pub const INTERFACE: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notify {
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    pub desktop_entry: String,
    /// The `transient` hint: not meant to stay in a history.
    pub transient: bool,
    /// The crash a "Crashes" notification is about (its x-maprisma-crash-* hints).
    pub crash: Option<crate::crash::Crash>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A Notify call from `sender` (its unique name) with this serial.
    Notify { sender: String, serial: u32, notify: Notify },
    /// A method return for `destination`'s call `reply_serial`; for a Notify, the server's id.
    Reply { destination: String, reply_serial: u32, server_id: Option<u32> },
    Closed { id: u32, reason: u32 },
    Action { id: u32, key: String },
}

type Args = (String, u32, String, String, String, Vec<String>, HashMap<String, OwnedValue>, i32);

fn number(value: &Value<'_>) -> Option<i64> {
    match value {
        Value::U8(v) => Some(*v as i64),
        Value::I16(v) => Some(*v as i64),
        Value::U16(v) => Some(*v as i64),
        Value::I32(v) => Some(*v as i64),
        Value::U32(v) => Some(*v as i64),
        Value::I64(v) => Some(*v),
        Value::U64(v) => i64::try_from(*v).ok(),
        Value::Value(inner) => number(inner),
        _ => None,
    }
}

fn string(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.to_string()),
        Value::Value(inner) => string(inner),
        _ => None,
    }
}

fn boolean(value: &Value<'_>) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Value(inner) => boolean(inner),
        other => number(other).map(|n| n != 0),
    }
}

/// The arguments of a Notify call; None when the body does not have Notify's signature.
pub fn parse_notify(message: &Message) -> Option<Notify> {
    let (app_name, replaces_id, app_icon, summary, body, _actions, hints, _timeout): Args = message.body().deserialize().ok()?;
    let hint = |key: &str| hints.get(key).map(|v| &**v);
    Some(Notify {
        app_name,
        replaces_id,
        app_icon,
        summary,
        body,
        urgency: hint("urgency").and_then(number).map_or(1, |u| u.clamp(0, 2) as u8),
        desktop_entry: hint("desktop-entry").and_then(string).unwrap_or_default(),
        transient: hint("transient").and_then(boolean).unwrap_or(false),
        crash: hint(crate::crash::HINT_PID).and_then(number).and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 0).map(|pid| {
            let text = |key: &str, limit: usize| crate::crash::clean(&hint(key).and_then(string).unwrap_or_default(), limit);
            crate::crash::Crash {
                pid,
                exe: text(crate::crash::HINT_EXE, 512),
                name: text(crate::crash::HINT_NAME, 64),
                signal: text(crate::crash::HINT_SIGNAL, 64),
                time: hint(crate::crash::HINT_TIME).and_then(number).unwrap_or(0),
            }
        }),
    })
}

/// The event in a message the monitor received, if it is one of ours.
pub fn event(message: &Message) -> Option<Event> {
    let header = message.header();
    match header.message_type() {
        Type::MethodCall => {
            if header.interface()?.as_str() != INTERFACE || header.member()?.as_str() != "Notify" {
                return None;
            }
            Some(Event::Notify {
                sender: header.sender().map(|s| s.to_string()).unwrap_or_default(),
                serial: header.primary().serial_num().get(),
                notify: parse_notify(message)?,
            })
        }
        Type::MethodReturn => Some(Event::Reply {
            destination: header.destination().map(|d| d.to_string()).unwrap_or_default(),
            reply_serial: header.reply_serial()?.get(),
            server_id: message.body().deserialize::<u32>().ok(),
        }),
        Type::Signal => {
            if header.interface()?.as_str() != INTERFACE {
                return None;
            }
            match header.member()?.as_str() {
                "NotificationClosed" => {
                    let (id, reason): (u32, u32) = message.body().deserialize().ok()?;
                    Some(Event::Closed { id, reason })
                }
                "ActionInvoked" => {
                    let (id, key): (u32, String) = message.body().deserialize().ok()?;
                    Some(Event::Action { id, key })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The app name shown and filtered by: Notify's app_name, else the desktop entry, else the icon.
pub fn app_label(n: &Notify) -> String {
    [&n.app_name, &n.desktop_entry, &n.app_icon]
        .into_iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty() && !s.contains('/'))
        .unwrap_or("")
        .to_owned()
}

/// The history entry for a notification (markup stripped, sizes capped, no images).
pub fn to_entry(n: &Notify, server_id: u32, now: i64) -> crate::history::Entry {
    use crate::history::{MAX_BODY, MAX_NAME, MAX_SUMMARY, cap, format_utc, new_id, strip_markup};
    let icon = if n.app_icon.trim().is_empty() { &n.desktop_entry } else { &n.app_icon };
    crate::history::Entry {
        id: new_id(),
        time: format_utc(now),
        app: cap(&app_label(n), MAX_NAME),
        icon: cap(icon.trim(), MAX_NAME),
        summary: cap(&strip_markup(&n.summary).replace('\n', " "), MAX_SUMMARY),
        body: cap(&strip_markup(&n.body), MAX_BODY),
        urgency: n.urgency,
        server_id,
        read: false,
        closed: false,
        // Only the recorder's own crash notifications carry a crash.
        crash: n.crash.clone().filter(|_| app_label(n) == crate::crash::APP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(hints: HashMap<&str, Value<'_>>) -> Message {
        Message::method_call(PATH, "Notify")
            .unwrap()
            .interface(INTERFACE)
            .unwrap()
            .destination(INTERFACE)
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .build(&("Firefox", 0u32, "firefox", "Hello <b>you</b>", "Line<br>two &amp; more", vec!["default", "Open"], hints, -1i32))
            .unwrap()
    }

    #[test]
    fn notify_call() {
        let mut hints = HashMap::new();
        hints.insert("urgency", Value::U8(2));
        hints.insert("desktop-entry", Value::from("org.mozilla.firefox"));
        hints.insert("image-data", Value::from(vec![1u8, 2, 3]));
        let message = call(hints);
        let Some(Event::Notify { sender, serial, notify }) = event(&message) else { panic!("not a Notify") };
        assert_eq!(sender, ":1.42");
        assert_eq!(serial, message.primary_header().serial_num().get());
        assert_eq!(notify.app_name, "Firefox");
        assert_eq!(notify.urgency, 2);
        assert_eq!(notify.desktop_entry, "org.mozilla.firefox");
        assert!(!notify.transient);
        let entry = to_entry(&notify, 7, 0);
        assert_eq!((entry.summary.as_str(), entry.body.as_str()), ("Hello you", "Line\ntwo & more"));
        assert_eq!((entry.app.as_str(), entry.icon.as_str(), entry.server_id), ("Firefox", "firefox", 7));
        assert_eq!(entry.time, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn hint_types_and_defaults() {
        let mut hints = HashMap::new();
        hints.insert("urgency", Value::I32(9));
        hints.insert("transient", Value::Bool(true));
        let n = parse_notify(&call(hints)).unwrap();
        assert_eq!(n.urgency, 2);
        assert!(n.transient);
        let n = parse_notify(&call(HashMap::new())).unwrap();
        assert_eq!(n.urgency, 1);
        // Not Notify's signature.
        let wrong = Message::method_call(PATH, "Notify").unwrap().interface(INTERFACE).unwrap().build(&("x", 1u32)).unwrap();
        assert_eq!(parse_notify(&wrong), None);
        assert_eq!(event(&wrong), None);
        // Another member or interface.
        let other = Message::method_call(PATH, "GetServerInformation").unwrap().interface(INTERFACE).unwrap().build(&()).unwrap();
        assert_eq!(event(&other), None);
    }

    #[test]
    fn reply_and_signals() {
        let message = call(HashMap::new());
        let reply = Message::method_return(&message.header()).unwrap().build(&(42u32,)).unwrap();
        assert_eq!(
            event(&reply),
            Some(Event::Reply { destination: ":1.42".into(), reply_serial: message.primary_header().serial_num().get(), server_id: Some(42) })
        );
        let closed = Message::signal(PATH, INTERFACE, "NotificationClosed").unwrap().build(&(42u32, 2u32)).unwrap();
        assert_eq!(event(&closed), Some(Event::Closed { id: 42, reason: 2 }));
        let action = Message::signal(PATH, INTERFACE, "ActionInvoked").unwrap().build(&(42u32, "default")).unwrap();
        assert_eq!(event(&action), Some(Event::Action { id: 42, key: "default".into() }));
        let other = Message::signal(PATH, "org.example.Other", "NotificationClosed").unwrap().build(&(1u32, 1u32)).unwrap();
        assert_eq!(event(&other), None);
    }

    #[test]
    fn crash_hints_reach_the_entry() {
        let crash = crate::crash::Crash { pid: 31, exe: "/usr/bin/cat".into(), name: "cat".into(), signal: "SIGSEGV".into(), time: 1_794_557_523 };
        let a = crate::crash::notify_args(&crash, 1, 0, true);
        let build = |app: &str| {
            Message::method_call(PATH, "Notify")
                .unwrap()
                .interface(INTERFACE)
                .unwrap()
                .build(&(app, a.replaces_id, a.app_icon.as_str(), a.summary.as_str(), a.body.as_str(), a.actions.clone(), a.hints.clone(), a.timeout))
                .unwrap()
        };
        let n = parse_notify(&build(crate::crash::APP)).unwrap();
        assert!(!n.transient);
        assert_eq!(n.crash.as_ref(), Some(&crash));
        let entry = to_entry(&n, 5, 0);
        assert_eq!((entry.app.as_str(), entry.icon.as_str(), entry.summary.as_str()), ("Crashes", "dialog-error", "cat crashed"));
        assert!(entry.body.starts_with("SIGSEGV\n/usr/bin/cat\n"), "{}", entry.body);
        assert_eq!(entry.crash, Some(crash));
        // Another app's hints don't make a crash entry.
        assert_eq!(to_entry(&parse_notify(&build("Other")).unwrap(), 5, 0).crash, None);
    }

    #[test]
    fn app_label_fallbacks() {
        let n = Notify { desktop_entry: "steam".into(), app_icon: "/usr/share/x.png".into(), ..Default::default() };
        assert_eq!(app_label(&n), "steam");
        let n = Notify { app_icon: "/usr/share/x.png".into(), ..Default::default() };
        assert_eq!(app_label(&n), "");
        let n = Notify { app_name: "  Mail ".into(), ..Default::default() };
        assert_eq!(app_label(&n), "Mail");
    }
}
