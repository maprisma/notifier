//! `settings.json` (docs/plan.md): every key optional, a missing or invalid one is its default;
//! writers change only their own keys and keep the rest ([set]).

use serde_json::Value;

pub const HISTORY_MAX: (u32, u32, u32) = (50, 500, 5000); // min, default, max
pub const HISTORY_DAYS: (u32, u32, u32) = (1, 7, 90);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub on: bool,
    /// Minutes after midnight, local time.
    pub from: u32,
    pub to: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self { on: false, from: 22 * 60, to: 7 * 60 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub dnd: bool,
    pub schedule: Schedule,
    pub history_max: u32,
    pub history_days: u32,
    pub muted_apps: Vec<String>,
    pub hidden_apps: Vec<String>,
    pub show_count: bool,
    pub language: String,
    pub corners: String,
    pub screenshot_key: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dnd: false,
            schedule: Schedule::default(),
            history_max: HISTORY_MAX.1,
            history_days: HISTORY_DAYS.1,
            muted_apps: vec![],
            hidden_apps: vec![],
            show_count: true,
            language: "system".into(),
            corners: "desktop".into(),
            screenshot_key: true,
        }
    }
}

/// "HH:MM" (00:00–23:59) as minutes after midnight.
pub fn parse_time(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_time(minutes: u32) -> String {
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

/// A whole number clamped to `(min, default, max)`; a fraction or not a number is the default
/// (the same rule as Maprisma Manager's Notifications page).
fn bounded(value: &Value, (min, default, max): (u32, u32, u32)) -> u32 {
    match value.as_f64().filter(|v| v.is_finite() && v.fract() == 0.0) {
        Some(v) => v.clamp(min as f64, max as f64) as u32,
        None => default,
    }
}

fn names(value: &Value) -> Vec<String> {
    let mut list: Vec<String> = value
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    list.dedup();
    list
}

pub fn parse(value: &Value) -> Settings {
    let d = Settings::default();
    let flag = |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
    let schedule = value.get("dnd_schedule").unwrap_or(&Value::Null);
    let time = |key: &str, default: u32| schedule.get(key).and_then(Value::as_str).and_then(parse_time).unwrap_or(default);
    let language = value.get("language").and_then(Value::as_str).unwrap_or("");
    let language = if crate::i18n::LANGUAGES.iter().any(|(c, _)| *c == language) { language } else { "system" };
    Settings {
        dnd: flag("dnd", d.dnd),
        schedule: Schedule {
            on: schedule.get("on").and_then(Value::as_bool).unwrap_or(d.schedule.on),
            from: time("from", d.schedule.from),
            to: time("to", d.schedule.to),
        },
        history_max: value.get("history_max").map_or(d.history_max, |v| bounded(v, HISTORY_MAX)),
        history_days: value.get("history_days").map_or(d.history_days, |v| bounded(v, HISTORY_DAYS)),
        muted_apps: value.get("muted_apps").map(names).unwrap_or_default(),
        hidden_apps: value.get("hidden_apps").map(names).unwrap_or_default(),
        show_count: flag("show_count", d.show_count),
        language: language.into(),
        corners: crate::corners::normalize(value.get("corners").and_then(Value::as_str).unwrap_or("")).into(),
        screenshot_key: flag("screenshot_key", d.screenshot_key),
    }
}

pub fn read_value() -> Value {
    std::fs::read(crate::paths::settings_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn load() -> Settings {
    parse(&read_value())
}

/// Sets (Some) or removes (None) one key; every other key stays as it is.
pub fn set(key: &str, value: Option<Value>) -> std::io::Result<()> {
    crate::paths::update_json(&crate::paths::settings_file(), |object| match value {
        Some(value) => {
            object.insert(key.into(), value);
        }
        None => {
            object.remove(key);
        }
    })
}

/// Whether `now` (minutes after midnight) lies in from–to; a window crossing midnight
/// (22:00–07:00) works, from == to is empty.
pub fn in_window(now: u32, from: u32, to: u32) -> bool {
    if from == to {
        false
    } else if from < to {
        (from..to).contains(&now)
    } else {
        now >= from || now < to
    }
}

/// Do not disturb in effect: the user's switch, or the schedule's window.
pub fn effective_dnd(settings: &Settings, now_minutes: u32) -> bool {
    settings.dnd || (settings.schedule.on && in_window(now_minutes, settings.schedule.from, settings.schedule.to))
}

/// The local time as minutes after midnight.
pub fn local_minutes() -> u32 {
    let (_, minutes) = local_day_and_minutes(now_unix());
    minutes
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// (local day number for comparing days, minutes after midnight) of a Unix time.
pub fn local_day_and_minutes(unix: i64) -> (i64, u32) {
    let t = local_tm(unix);
    let day = crate::history::days_from_civil(t.tm_year as i64 + 1900, t.tm_mon as u32 + 1, t.tm_mday as u32);
    (day, (t.tm_hour * 60 + t.tm_min) as u32)
}

pub fn local_tm(unix: i64) -> libc::tm {
    // SAFETY: localtime_r writes only into `tm`, which lives on this stack.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t: libc::time_t = unix as libc::time_t;
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_for_missing_and_broken() {
        assert_eq!(parse(&json!({})), Settings::default());
        assert_eq!(parse(&json!("nonsense")), Settings::default());
        let broken = json!({"dnd": "yes", "dnd_schedule": {"on": 1, "from": "25:00", "to": "7"},
            "history_max": "many", "history_days": null, "muted_apps": "Firefox", "show_count": 0,
            "language": "xx", "corners": "wobbly", "screenshot_key": "no"});
        assert_eq!(parse(&broken), Settings::default());
    }

    #[test]
    fn values_and_clamps() {
        let s = parse(&json!({"dnd": true, "dnd_schedule": {"on": true, "from": "21:30", "to": "6:05"},
            "history_max": 10, "history_days": 400, "muted_apps": ["Firefox", " ", 3, "Steam"],
            "hidden_apps": ["Spotify"], "show_count": false, "language": "de", "corners": "square",
            "screenshot_key": false, "window_size": {"width": 500, "height": 600}, "unknown": 1}));
        assert!(s.dnd);
        assert_eq!(s.schedule, Schedule { on: true, from: 21 * 60 + 30, to: 6 * 60 + 5 });
        assert_eq!(s.history_max, 50);
        assert_eq!(s.history_days, 90);
        assert_eq!(s.muted_apps, vec!["Firefox", "Steam"]);
        assert_eq!(s.hidden_apps, vec!["Spotify"]);
        assert!(!s.show_count && !s.screenshot_key);
        assert_eq!((s.language.as_str(), s.corners.as_str()), ("de", "square"));
        assert_eq!(parse(&json!({"history_max": 99999, "history_days": 0})).history_max, 5000);
        assert_eq!(parse(&json!({"history_days": 0})).history_days, 1);
        assert_eq!(parse(&json!({"history_max": 120.4})).history_max, 500);
        assert_eq!(parse(&json!({"history_max": 120.0, "history_days": 2.5})).history_max, 120);
        assert_eq!(parse(&json!({"history_days": 2.5})).history_days, 7);
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("00:00"), Some(0));
        assert_eq!(parse_time("23:59"), Some(1439));
        assert_eq!(parse_time("7:00"), Some(420));
        for bad in ["24:00", "12:60", "12", "", "ab:cd", "1:2", "123:00", "-1:00"] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
        assert_eq!(format_time(420), "07:00");
        assert_eq!(format_time(1439), "23:59");
    }

    #[test]
    fn schedule_window_and_midnight() {
        let (from, to) = (22 * 60, 7 * 60);
        assert!(in_window(22 * 60, from, to));
        assert!(in_window(23 * 60 + 59, from, to));
        assert!(in_window(0, from, to));
        assert!(in_window(6 * 60 + 59, from, to));
        assert!(!in_window(7 * 60, from, to));
        assert!(!in_window(12 * 60, from, to));
        assert!(!in_window(21 * 60 + 59, from, to));
        // Within one day.
        assert!(in_window(13 * 60, 12 * 60, 14 * 60));
        assert!(!in_window(14 * 60, 12 * 60, 14 * 60));
        assert!(!in_window(11 * 60, 12 * 60, 14 * 60));
        // Empty window.
        assert!(!in_window(5, 300, 300));
    }

    #[test]
    fn effective_state() {
        let mut s = Settings::default();
        assert!(!effective_dnd(&s, 23 * 60));
        s.schedule.on = true;
        assert!(effective_dnd(&s, 23 * 60));
        assert!(!effective_dnd(&s, 12 * 60));
        s.dnd = true;
        assert!(effective_dnd(&s, 12 * 60));
    }

    #[test]
    fn set_keeps_other_keys() {
        crate::paths::test_roots();
        let path = crate::paths::settings_file();
        let _guard = crate::history::tests::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"window_size": {"width": 500, "height": 600}, "x-other": [1]}"#).unwrap();
        set("dnd", Some(true.into())).unwrap();
        set("corners", None).unwrap();
        let v = read_value();
        assert_eq!(v["dnd"], true);
        assert_eq!(v["window_size"]["width"], 500);
        assert_eq!(v["x-other"][0], 1);
        assert!(load().dnd);
        set("dnd", None).unwrap();
        assert!(!load().dnd);
    }
}
