# Maprisma Notifier — plan and contract

A notification centre for every desktop (COSMIC, KDE, Hyprland): history, unread count in the bar,
do-not-disturb. It is **not** the notification server: COSMIC (`cosmic-notifications`), KDE
(Plasma) and Hyprland (Maprisma Shell) keep showing the popups. Rules: the `maprisma-layout` and
`maprisma-app` skills (`~/.claude/skills/`). Closest sibling to copy from:
`/mnt/Data/Projects/maprisma-llm-monitor` (Rust/libcosmic applet + `--window` + SNI tray, shared
`look.rs`, `corners.rs`, `shot.rs`, `window_size.rs`, i18n with 8 languages).

User decisions (2026-11-13): Maprisma Shell keeps the popups on Hyprland; the repo goes public as
`maprisma/notifier` once installed and tested.

## Parts

| Part | Binary | Does |
|---|---|---|
| recorder | `maprisma-notifier-recorder` (or inside the tray process) | Session D-Bus monitor (`org.freedesktop.DBus.Monitoring.BecomeMonitor`, match `interface='org.freedesktop.Notifications',member='Notify'` and `NotificationClosed`/`ActionInvoked` signals), read-only; appends to the history; applies the do-not-disturb schedule. One per session (D-Bus name `io.github.ubruckhaus.CosmicMaprismaNotifier.Recorder`, DoNotQueue). Started by the tray (KDE/Hyprland) or the applet (COSMIC); ends with its parent (PR_SET_PDEATHSIG) unless started standalone. |
| applet | `maprisma-notifier` | COSMIC applet `io.github.ubruckhaus.CosmicMaprismaNotifier` (popup) and `--window` (libcosmic window for KDE/Hyprland; a second `--window` start closes the open one via `io.github.ubruckhaus.CosmicMaprismaNotifier.Window` / interface `io.github.ubruckhaus.Notifier.Window`, method `Close`). |
| tray | `maprisma-notifier-tray` | SNI tray (KDE, Hyprland): icon bell / bell with dot (unread) / bell-off (do not disturb), tooltip "N unread"; left = toggle window, middle = toggle do-not-disturb, right = menu (Open, Do not disturb ✓, Clear all, Settings… → `maprisma-manager --page notifications`). |

## Files (the contract the Manager and Shell rely on)

### Settings — `$XDG_CONFIG_HOME/maprisma-notifier/settings.json`

```json
{
  "dnd": false,
  "dnd_schedule": { "on": false, "from": "22:00", "to": "07:00" },
  "history_max": 500,
  "history_days": 7,
  "muted_apps": [],
  "hidden_apps": [],
  "show_count": true,
  "language": "system",
  "corners": "desktop",
  "screenshot_key": true,
  "window_size": { "width": 420, "height": 640 }
}
```

- Every key optional; missing/invalid → the default above. Writers change only their keys
  (read–modify–write, atomic tmp + rename, keep unknown keys).
- `dnd` is the user's switch; the effective state is `dnd || (schedule on && now in from–to)`
  (a window crossing midnight works).
- `muted_apps`: app names (the Notify `app_name`) whose notifications are recorded but never count
  as unread. `hidden_apps`: not recorded at all.
- `history_max` 50–5000, `history_days` 1–90.

### History — `$XDG_STATE_HOME/maprisma-notifier/history.jsonl`

One JSON object per line, appended; compacted (oldest dropped) when over `history_max` or older
than `history_days`:

```json
{"id": "<uuid-ish>", "time": "2026-11-13T08:12:03Z", "app": "Firefox", "icon": "firefox",
 "summary": "…", "body": "… (plain text, markup stripped, ≤ 2000 chars)", "urgency": 1,
 "server_id": 42, "read": false, "closed": false}
```

`read` is changed by rewriting the file (atomic). Max file 4 MB. Images are not stored.

### Do not disturb — applied per desktop by the recorder (and the Manager)

| Desktop | Where |
|---|---|
| Hyprland | `~/.config/maprisma-shell/shell.json` `"notifications": {"dnd": bool}` (Shell watches the file) |
| COSMIC | cosmic-config `com.system76.CosmicNotifications` v1 key `do_not_disturb` (`~/.config/cosmic/com.system76.CosmicNotifications/v1/do_not_disturb`, `true`/`false`) |
| KDE | `org.freedesktop.Notifications.Inhibit` held by the recorder while effective (`UnInhibit` when it ends) |

### Maprisma Shell — `~/.config/maprisma-shell/shell.json` `"notifications"`

```json
"notifications": { "dnd": false, "position": "top-right", "timeout": 5, "max": 5 }
```

- `position`: `top-right` (default), `top`, `top-left`, `bottom-right`, `bottom-left`.
- `timeout`: seconds a normal popup stays, 1–60 (critical ones stay until dismissed).
- `max`: popups shown at once, 1–10 (the oldest leave first).
- Shell reads it live (FileView watch), persists nothing itself except through this file; its IPC
  gets `shell setDnd <true|false>` / `shell getDnd` which write/read the same key. Writers change
  only their keys (the Manager's power.dart pattern: re-read before write, keep the rest).

## Maprisma Manager

- Catalog entry `notifier` ("Maprisma Notifier", kind `rust`, repo `maprisma-notifier`, GitHub
  `maprisma/notifier`; bar icon: COSMIC applet `io.github.ubruckhaus.CosmicMaprismaNotifier`,
  KDE tray `maprisma-notifier` with autostart `OnlyShowIn=KDE;`, Hyprland via the same tray; data
  `{config}/maprisma-notifier`, `{state}/maprisma-notifier`).
- Settings category **Notifications** (sidebar, between Sound and Power): do not disturb (switch +
  schedule from/to), history length (entries, days), show the unread count, muted and hidden apps
  (from the history's app names), *Clear history*; on Hyprland also Shell's popup position,
  timeout and max; on COSMIC/KDE a button that opens the desktop's own notification settings
  (`cosmic-settings notifications` / `systemsettings kcm_notifications`). Reads/writes the files
  above (changed keys only). When Maprisma Notifier isn't installed: a short note with Install.

## Order

1. recorder + history + settings (pure functions tested; D-Bus monitor tested under
   `dbus-run-session`).
2. window/applet + tray (Esc, toggle, window size, corners, Language, F12; 8 languages).
3. do not disturb on the three desktops + schedule.
4. Shell `notifications` keys + IPC (maprisma-shell repo, its qml tests).
5. Manager catalog entry + Notifications page (8 languages).
6. Off-screen screenshots, install through the Manager, live test on Hyprland, publish.
