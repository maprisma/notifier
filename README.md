# Maprisma Notifier

**A notification centre for COSMIC, KDE Plasma and Hyprland: what your apps told you, how much
is unread, and do not disturb, on a switch or a schedule.**

One of the [Maprisma](https://github.com/maprisma) apps (Rust/libcosmic). It is **not** a
notification server: COSMIC (`cosmic-notifications`), Plasma and Maprisma Shell (Hyprland) keep
showing the popups. Maprisma Notifier records what they show and keeps it in a history.

- **Bar icon:** a bell; a bell with a dot while something is unread (on COSMIC with the count next
  to it); a crossed-out bell while do not disturb is on.
- **Popup / window:** the history grouped by app (the app with the newest notification first,
  newest first inside), an unread dot before new ones (red for critical ones; a click marks one
  read), *Clear* per app, *Mark all read*, *Clear all* (asks first), the **Do not disturb** switch
  with the schedule, Language, Corners, *F12 saves a screenshot* and *Notification settings…*
  (Maprisma Manager's Notifications page).
- **Do not disturb:** the switch, or a daily schedule (e.g. 22:00–07:00, across midnight), see
  below.

Languages: English, Deutsch, Français, Español, Italiano, Português, Nederlands, Polski (*Language*
in the popup or the tray menu; *System* follows the locale).

## Desktops

| Desktop | Bar | Click | Do not disturb goes to |
|---|---|---|---|
| COSMIC | applet `io.github.ubruckhaus.CosmicMaprismaNotifier` | left: popup, middle: do not disturb | cosmic-config `com.system76.CosmicNotifications` v1 `do_not_disturb` |
| KDE Plasma | tray `maprisma-notifier-tray` (autostart) | left: window, middle: do not disturb, right: menu | `org.freedesktop.Notifications.Inhibit`, held while on |
| Hyprland | the same tray in Maprisma Shell's bar | as on KDE | `~/.config/maprisma-shell/shell.json` `notifications.dnd` |

The tray menu: *Open Maprisma Notifier*, *Do not disturb* ✓, *Clear all*, *Notification
settings…*, *Language*, *Quit tray*.

## How it works

`maprisma-notifier-recorder` watches the session bus **read-only** (D-Bus
`BecomeMonitor` with filters for `org.freedesktop.Notifications.Notify` calls, the server's
replies and its `NotificationClosed` / `ActionInvoked` signals). It never answers, changes or
sends a notification. It runs once per session (D-Bus name
`io.github.ubruckhaus.CosmicMaprismaNotifier.Recorder`); the applet (COSMIC) or the tray (KDE,
Hyprland) starts it and it ends with them.

- Each notification becomes one line in the history: app, icon name, title, text (markup removed,
  at most 2000 characters), urgency, the server's id. **Images are not stored.** Notifications
  marked *transient* (volume, brightness) and apps in `hidden_apps` are not recorded; a
  notification that replaces another updates its line. Clicking an action in a popup marks it read.
- Do not disturb is in effect when the switch is on **or** the schedule is on and the time is in
  its window. The recorder writes it to the desktop (table above) **only when that changes** (the
  switch, a schedule boundary); a change made on the desktop itself stays until the next change.
  On KDE the inhibition ends with the recorder.

## Files

| File | What |
|---|---|
| `~/.config/maprisma-notifier/settings.json` | settings, shared with Maprisma Manager (every key optional; writers keep the others) |
| `~/.local/state/maprisma-notifier/history.jsonl` | the history, one JSON object per line, `0600` |
| `~/.local/state/maprisma-notifier/history.lock` | held while the history is rewritten |
| `~/Pictures/Maprisma Notifier/` | F12 screenshots and their notes |

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

- `history_max` 50–5000 entries, `history_days` 1–90; the oldest go first, and the file stays
  under 4 MB. Whole numbers only; anything else is the default.
- `muted_apps`: recorded, but never counted as unread. `hidden_apps`: not recorded.
- `dnd_schedule` with `from` equal to `to` is no window.

## Install

Needs Rust (edition 2024) and the libraries libcosmic builds against (Wayland, xkbcommon).

```sh
./install.sh
```

It installs into `~/.local`:

- `bin/maprisma-notifier`, `bin/maprisma-notifier-tray`, `bin/maprisma-notifier-recorder`
- `share/applications/io.github.ubruckhaus.CosmicMaprismaNotifier.desktop` (the COSMIC applet) and
  `….Window.desktop` (the window)
- `share/icons/hicolor/scalable/apps/io.github.ubruckhaus.CosmicMaprismaNotifier{,-unread,-dnd}-symbolic.svg`
- `~/.config/autostart/io.github.ubruckhaus.CosmicMaprismaNotifier.Tray.desktop`
  (`OnlyShowIn=KDE;Hyprland;`)

On COSMIC add **Maprisma Notifier** in COSMIC Settings › Desktop › Panel › Applets. On KDE and
Hyprland the tray starts with the next login (or run `maprisma-notifier-tray`). Maprisma Manager
can install, update and remove it too.

## Development

```sh
unset DISPLAY
cargo test --release                                         # unit tests
dbus-run-session -- cargo test --release -- --ignored        # recorder and window on a private bus
tools/gen-translations                                       # after changing translations/*.json
```

`src/look.rs`, `src/corners.rs`, `src/shot.rs` and `src/window_size.rs` are shared copies, the
same file in every Maprisma Rust app; change them in all of them.

## License

MIT, see [LICENSE](LICENSE).
