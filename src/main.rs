// Maprisma Notifier: the notification history, the unread count in the bar and do not disturb.
// COSMIC panel applet (popup) and `--window` (KDE, Hyprland: opened by the tray). The desktop's
// own server keeps showing the popups; the recorder (maprisma-notifier-recorder) writes what
// it sees to history.jsonl, which this reads every two seconds.

use cosmic::app::ApplicationExt;
use maprisma_notifier::history::{self, Entry};
use maprisma_notifier::i18n::{language_options, read_pref, set_language, tr, trf, write_pref};
use maprisma_notifier::settings::{self, Settings};
use maprisma_notifier::{corners, shot};
mod window_mode;
mod window_size;
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use cosmic::app::{Core, Task};
use cosmic::applet::cosmic_panel_config::PanelAnchor;
use cosmic::applet::{menu_button, padded_control};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::destroy_popup;
use cosmic::iced::widget::{container as iced_container, text as iced_text};
use cosmic::iced::{Alignment, Background, Border, Color, Length, Rectangle, Subscription, window};
use cosmic::widget::rectangle_tracker::{RectangleTracker, RectangleUpdate, rectangle_tracker_subscription};
use cosmic::widget::{Id, autosize, button, column, container, divider, icon, mouse_area, row, scrollable, space, text};
use cosmic::{Element, theme};
use serde_json::Value;

const APP_ID: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier";
const ICON: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-symbolic";
const ICON_UNREAD: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-unread-symbolic";
const ICON_DND: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-dnd-symbolic";
static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("notifier-autosize"));
static NOTE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("notifier-shot-note"));
// F12 pictures go to ~/Pictures/<this>/ (the app's name, the same in every language).
const SHOT_FOLDER: &str = "Maprisma Notifier";

const POPUP_WIDTH: f32 = 400.0;
const POPUP_LIST_HEIGHT: f32 = 460.0;
const DEFAULT_SIZE: (f32, f32) = (420.0, 640.0);
/// Rows drawn per app (the history may hold thousands).
const SHOWN_PER_APP: usize = 50;
const BODY_SHOWN: usize = 300;

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|arg| arg == "--window") {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("D-Bus runtime");
        let Some(_connection) = runtime.block_on(window_mode::claim()).expect("Window D-Bus connection") else {
            // A second `--window` start toggles (layout rule 5a): the open window closes.
            runtime.block_on(window_mode::close_running());
            return Ok(());
        };
        cosmic::app::run::<Notifier<true>>(
            cosmic::app::Settings::default()
                .size({
                    let (width, height) = window_start_size();
                    cosmic::iced::Size::new(width, height)
                })
                .resizable(Some(8.0))
                .transparent(false)
                .is_daemon(false),
            (),
        )
    } else {
        // COSMIC: the applet keeps the recorder running (KDE and Hyprland: the tray does).
        maprisma_notifier::recorder::supervise();
        cosmic::applet::run::<Notifier<false>>(())
    }
}

// The `--window` size: the one the user left it at (layout rule 5a), else the default.
fn window_start_size() -> window_size::Size {
    window_size::parse(read_pref(window_size::KEY).as_ref()).unwrap_or(DEFAULT_SIZE)
}

// ------------------------------------------------------------------------- formatting

/// "08:12" today, "Yesterday 08:12", else "2026-11-10 08:12" (local time).
fn when(entry: &Entry, today: i64) -> String {
    let Some(unix) = history::parse_utc(&entry.time) else { return String::new() };
    let (day, minutes) = settings::local_day_and_minutes(unix);
    let clock = settings::format_time(minutes);
    if day == today {
        clock
    } else if day == today - 1 {
        trf("Yesterday {0}", &[clock])
    } else {
        let t = settings::local_tm(unix);
        format!("{:04}-{:02}-{:02} {clock}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday)
    }
}

/// The line under the title: unread count and do not disturb.
fn status_line(unread: usize, dnd: bool) -> String {
    let count = match unread {
        0 => tr("No unread notifications"),
        1 => tr("1 unread"),
        n => trf("{0} unread", &[n.to_string()]),
    };
    if dnd { format!("{count} · {}", tr("Do not disturb")) } else { count }
}

// ------------------------------------------------------------------------- widgets

fn dim_text(theme: &cosmic::Theme) -> iced_text::Style {
    let mut color = Color::from(theme.cosmic().on_bg_color());
    color.a = 0.6;
    iced_text::Style { color: Some(color), ..Default::default() }
}

fn accent_text(theme: &cosmic::Theme) -> iced_text::Style {
    iced_text::Style { color: Some(Color::from(theme.cosmic().accent_text_color())), ..Default::default() }
}

fn dim<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a, Message> {
    text::caption(content).class(theme::Text::Custom(dim_text)).into()
}

fn section<'a>(title: String) -> Element<'a, Message> {
    text::caption_heading(title.to_uppercase()).class(theme::Text::Custom(dim_text)).into()
}

/// The unread mark: an accent dot (red for a critical one); muted or read: none.
fn mark<'a>(entry: &Entry, muted: bool) -> Element<'a, Message> {
    let shown = !entry.read && !muted;
    let critical = entry.urgency >= 2;
    let radius = corners::now().pill;
    let style = theme::Container::custom(move |theme: &cosmic::Theme| {
        let color = if !shown {
            Color::TRANSPARENT
        } else if critical {
            Color::from(theme.cosmic().destructive_color())
        } else {
            Color::from(theme.cosmic().accent_color())
        };
        iced_container::Style { background: Some(Background::Color(color)), border: Border { radius: radius.into(), ..Default::default() }, ..Default::default() }
    });
    container(space::horizontal()).width(Length::Fixed(8.0)).height(Length::Fixed(8.0)).class(style).into()
}

// ------------------------------------------------------------------------- the app

#[derive(Default)]
struct Notifier<const WINDOW_MODE: bool> {
    core: Core,
    window_mode: bool,
    popup: Option<window::Id>,
    rectangle: Rectangle,
    tracker: Option<RectangleTracker<u32>>,
    entries: Vec<Entry>,
    settings: Settings,
    history_stamp: Option<(SystemTime, u64)>,
    settings_stamp: Option<(SystemTime, u64)>,
    dnd: bool,
    confirm_clear: bool,
    language_open: bool,
    language_error: bool,
    // F12: the note's state, whether a capture is on its way.
    shot: shot::Phase,
    capturing: bool,
    corners_open: bool,
    setting_error: bool,
    resize: window_size::Debounce,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    SaveSize,
    TogglePopup,
    CloseRequested(window::Id),
    Rectangle(RectangleUpdate<u32>),
    CheckClose,
    SetDnd(bool),
    ToggleDnd,
    MarkRead(String),
    MarkAllRead,
    ClearApp(String),
    AskClearAll,
    CancelClear,
    ClearAll,
    OpenSettings,
    Language(String),
    ToggleLanguage,
    ShotKey(window::Id),
    Escape(window::Id),
    Captured(window::Screenshot),
    ShotSaved(Result<shot::Saved, String>),
    NoteInput(String),
    NoteDone,
    NoteSaved(Result<(), String>),
    SetShotKey(bool),
    ToggleCorners,
    Corners(String),
}

/// What Esc does (layout rule 5a): the innermost open thing first, then the window or popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscStep {
    /// Skip the screenshot's note (or close its error).
    Note,
    /// Leave "Clear all?" without clearing.
    Confirm,
    /// Close the open Language / Corners list.
    Menus,
    /// Close the window (`--window`) or the popup.
    Close,
    Nothing,
}

/// `note`: the screenshot card shows; `confirm`: "Clear all?" asks; `menus`: a list is open;
/// `shown`: Esc came from the window or the open popup (not the panel button).
fn esc_step(note: bool, confirm: bool, menus: bool, shown: bool) -> EscStep {
    if !shown {
        EscStep::Nothing
    } else if note {
        EscStep::Note
    } else if confirm {
        EscStep::Confirm
    } else if menus {
        EscStep::Menus
    } else {
        EscStep::Close
    }
}

/// Which bar icon: do not disturb wins, then unread.
fn bar_icon(dnd: bool, unread: usize) -> &'static str {
    if dnd {
        ICON_DND
    } else if unread > 0 {
        ICON_UNREAD
    } else {
        ICON
    }
}

// Runs `work` on its own thread (saving must not hold up the popup) and answers with `done`.
fn off_thread<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static, done: fn(Result<T, String>) -> Message) -> Task<Message> {
    let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    Task::perform(receiver, move |result| cosmic::Action::App(done(result.unwrap_or_else(|_| Err("the file could not be written".into())))))
}

fn stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

// Opens Maprisma Manager's Notifications page (the full settings), without waiting for it.
fn open_manager() {
    if let Ok(mut child) = std::process::Command::new("maprisma-manager")
        .args(["--page", "notifications"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

impl<const WINDOW_MODE: bool> Notifier<WINDOW_MODE> {
    fn unread(&self) -> usize {
        let visible: Vec<Entry> = self.entries.iter().filter(|e| !self.settings.hidden_apps.contains(&e.app)).cloned().collect();
        history::unread_count(&visible, &self.settings.muted_apps)
    }

    /// Re-reads settings and history when their files changed; the effective do not disturb
    /// (the schedule moves with the clock). Returns whether the look must be set again.
    fn reload(&mut self, force: bool) -> bool {
        let mut restyle = false;
        let settings_stamp = stamp(&maprisma_notifier::paths::settings_file());
        if force || settings_stamp != self.settings_stamp {
            self.settings_stamp = settings_stamp;
            let fresh = settings::load();
            if fresh.corners != self.settings.corners {
                corners::set_current(&fresh.corners);
                restyle = true;
            }
            self.settings = fresh;
        }
        let history_stamp = stamp(&maprisma_notifier::paths::history_file());
        if force || history_stamp != self.history_stamp {
            self.history_stamp = history_stamp;
            self.entries = history::load();
        }
        self.dnd = settings::effective_dnd(&self.settings, settings::local_minutes());
        restyle
    }

    /// After a change of ours: the files again at once.
    fn changed(&mut self, result: std::io::Result<bool>) {
        self.setting_error = result.is_err();
        self.reload(true);
    }

    fn panel_content(&self) -> Element<'_, Message> {
        let unread = self.unread();
        let icon_size = self.core.applet.suggested_size(true).0;
        let bell = icon::from_name(bar_icon(self.dnd, unread)).size(icon_size).symbolic(true).icon();
        if !self.settings.show_count || unread == 0 || self.dnd {
            return bell.into();
        }
        let label = self.core.applet.text(if unread > 99 { "99+".to_string() } else { unread.to_string() }).class(theme::Text::Custom(accent_text));
        if matches!(self.core.applet.anchor, PanelAnchor::Top | PanelAnchor::Bottom) {
            row![bell, label].spacing(4).align_y(Alignment::Center).into()
        } else {
            column![bell, label].spacing(2).align_x(Alignment::Center).into()
        }
    }

    fn header(&self) -> Element<'_, Message> {
        let unread = self.unread();
        row![
            icon::from_name(bar_icon(self.dnd, unread)).size(32).symbolic(true).icon(),
            column![text::title4(tr("Maprisma Notifier")), dim(status_line(unread, self.dnd))].spacing(2),
            space::horizontal().width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    fn dnd_row(&self) -> Element<'_, Message> {
        let s = &self.settings.schedule;
        let mut words = column![text::body(tr("Do not disturb"))].spacing(2);
        if s.on {
            let mut line = trf("Scheduled {0}–{1}", &[settings::format_time(s.from), settings::format_time(s.to)]);
            if self.dnd && !self.settings.dnd {
                line = format!("{line} · {}", tr("on now"));
            }
            words = words.push(dim(line));
        }
        row![words, space::horizontal().width(Length::Fill), cosmic::widget::toggler(self.settings.dnd).on_toggle(Message::SetDnd)]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
    }

    fn entry_row<'a>(&self, entry: &'a Entry, muted: bool, today: i64) -> Element<'a, Message> {
        let unread = !entry.read && !muted;
        let summary = if entry.summary.is_empty() { tr("(no title)") } else { entry.summary.clone() };
        let summary: Element<Message> = if unread { text::heading(summary).into() } else { text::body(summary).into() };
        let mut words = column![row![container(summary).width(Length::Fill), dim(when(entry, today))].spacing(8).align_y(Alignment::Start)].spacing(2);
        if !entry.body.is_empty() {
            words = words.push(dim(history::cap(&entry.body, BODY_SHOWN)));
        }
        let content = row![container(mark(entry, muted)).padding([6, 0, 0, 0]), words].spacing(8).align_y(Alignment::Start);
        let row_button = menu_button(content).padding([4, 8]);
        if unread { row_button.on_press(Message::MarkRead(entry.id.clone())).into() } else { row_button.into() }
    }

    fn history_view(&self) -> Element<'_, Message> {
        let groups = history::by_app(&self.entries, &self.settings.hidden_apps);
        if groups.is_empty() {
            // Empty state: message, a muted line, one action.
            return container(
                column![
                    text::body(tr("No notifications")),
                    dim(tr("Notifications from your apps appear here.")),
                    button::text(tr("Notification settings")).on_press(Message::OpenSettings),
                ]
                .spacing(8)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([24, 16])
            .align_x(Alignment::Center)
            .into();
        }
        let today = settings::local_day_and_minutes(settings::now_unix()).0;
        let mut list = column![].spacing(12);
        for (app, entries) in groups {
            let muted = self.settings.muted_apps.contains(&app);
            let name = if app.is_empty() { tr("Other") } else { app.clone() };
            let heading = if muted { format!("{name} · {}", tr("muted")) } else { name };
            let mut group = column![
                row![section(heading), space::horizontal().width(Length::Fill), button::text(tr("Clear")).on_press(Message::ClearApp(app.clone()))]
                    .align_y(Alignment::Center)
                    .padding([0, 0, 0, 8])
            ]
            .spacing(2);
            for entry in entries.iter().take(SHOWN_PER_APP) {
                group = group.push(self.entry_row(entry, muted, today));
            }
            if entries.len() > SHOWN_PER_APP {
                group = group.push(container(dim(trf("{0} older ones", &[(entries.len() - SHOWN_PER_APP).to_string()]))).padding([0, 8]));
            }
            list = list.push(group);
        }
        list.into()
    }

    fn actions(&self) -> Element<'_, Message> {
        let mut actions = column![divider::horizontal::default()];
        if self.confirm_clear {
            actions = actions.push(padded_control(
                column![
                    text::body(tr("Clear all notifications?")),
                    row![
                        space::horizontal().width(Length::Fill),
                        button::standard(tr("Cancel")).on_press(Message::CancelClear),
                        button::destructive(tr("Clear all")).on_press(Message::ClearAll),
                    ]
                    .spacing(8),
                ]
                .spacing(8),
            ));
        } else {
            actions = actions.push(menu_button(text::body(tr("Mark all read"))).on_press_maybe((self.unread() > 0).then_some(Message::MarkAllRead)));
            actions = actions.push(menu_button(text::body(tr("Clear all"))).on_press_maybe((!self.entries.is_empty()).then_some(Message::AskClearAll)));
        }
        actions = actions.push(divider::horizontal::default());
        actions = actions.push(menu_button(text::body(tr("Language"))).on_press(Message::ToggleLanguage));
        if self.language_error {
            actions = actions.push(text::body(tr("Could not save the language setting.")));
        }
        if self.language_open {
            for (code, label) in language_options() {
                actions = actions.push(menu_button(text::body(label)).on_press(Message::Language(code)));
            }
            return actions.into();
        }
        actions = actions.push(padded_control(
            row![text::body(tr("F12 saves a screenshot")), space::horizontal().width(Length::Fill), cosmic::widget::toggler(self.settings.screenshot_key).on_toggle(Message::SetShotKey)]
                .align_y(Alignment::Center),
        ));
        actions = actions.push(menu_button(text::body(tr("Corners"))).on_press(Message::ToggleCorners));
        if self.corners_open {
            let desktop = corners::desktop_radius().round().to_string();
            for (choice, label) in [
                ("desktop", trf("Like the desktop ({0} px)", &[desktop])),
                ("slight", trf("Slightly rounded ({0} px)", &["6".into()])),
                ("rounded", trf("Rounded ({0} px)", &["12".into()])),
                ("square", tr("Square")),
            ] {
                let label = if self.settings.corners == choice { format!("✓ {label}") } else { label };
                actions = actions.push(menu_button(text::body(label)).on_press(Message::Corners(choice.into())));
            }
        }
        actions = actions.push(menu_button(text::body(tr("Notification settings…"))).on_press(Message::OpenSettings));
        if self.setting_error {
            actions = actions.push(text::body(tr("Could not save the setting.")));
        }
        actions.into()
    }

    fn popup_content(&self) -> Element<'_, Message> {
        if let Some(overlay) = self.shot_overlay() {
            return overlay;
        }
        let top = column![self.header(), divider::horizontal::default(), self.dnd_row(), divider::horizontal::default()].spacing(8);
        let top = container(top).padding([12, 16, 4, 16]).width(Length::Fill);
        let list = scrollable(container(self.history_view()).padding([4, 8]).width(Length::Fill));
        let list: Element<Message> = if self.window_mode {
            list.height(Length::Fill).into()
        } else {
            container(list.height(Length::Shrink)).max_height(POPUP_LIST_HEIGHT).into()
        };
        let actions = self.actions();
        let actions: Element<Message> = if self.language_open && !self.window_mode {
            scrollable(actions).height(Length::Fixed(320.0)).into()
        } else {
            actions
        };
        column![top, list, actions]
            .width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
            .padding([0, 0, 8, 0])
            .into()
    }

    // After F12: "Screenshot saved", where, the optional note and Done (or the error and Done).
    fn shot_overlay(&self) -> Option<Element<'_, Message>> {
        let body: Element<Message> = match &self.shot {
            shot::Phase::Note { shown, text: note, .. } => column![
                text::heading(tr("Screenshot saved")),
                dim(trf("Saved as {0}", &[shown.clone()])),
                cosmic::widget::text_input(tr("What is wrong? (optional)"), note.as_str()).id(NOTE_ID.clone()).on_input(Message::NoteInput).on_submit(|_| Message::NoteDone),
                row![space::horizontal().width(Length::Fill), button::suggested(tr("Done")).on_press(Message::NoteDone)],
            ]
            .spacing(10)
            .into(),
            shot::Phase::Error(reason) => column![text::body(reason.clone()), row![space::horizontal().width(Length::Fill), button::suggested(tr("Done")).on_press(Message::NoteDone)]]
                .spacing(10)
                .into(),
            _ => return None,
        };
        let card = container(body).padding(16).width(Length::Fill).class(theme::Container::Card);
        let card = container(card).padding(12).width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) });
        Some(card.into())
    }

    // Done, Enter or the popup closing (keep: the note typed so far is saved), Esc (skip).
    fn finish_note(&mut self, keep: bool) -> Task<Message> {
        match std::mem::take(&mut self.shot) {
            shot::Phase::Note { path, text, .. } => {
                if keep && shot::note_text(&text).is_some() {
                    // Busy until the note is written, so a new F12 cannot replace it.
                    self.shot = shot::Phase::Noting;
                    return off_thread(move || shot::save_note(&path, SHOT_FOLDER, &text).map(|_| ()).map_err(|e| e.to_string()), Message::NoteSaved);
                }
            }
            shot::Phase::Error(_) => {}
            other => self.shot = other,
        }
        Task::none()
    }

    // A resize that has settled (or, on `closing`, any waiting one) goes to settings.json; not
    // while maximised, fullscreen or tiled (libcosmic's sharp corners; hyprctl on Hyprland).
    fn save_size(&mut self, closing: bool) {
        let sharp = self.core.window.sharp_corners;
        let ignore = || window_size::not_users_size(sharp);
        let size = if closing { self.resize.flush(ignore) } else { self.resize.due(std::time::Instant::now(), ignore) };
        if let Some(size) = size {
            self.setting_error = write_pref(window_size::KEY, Some(window_size::to_value(size))).is_err();
        }
    }

    // A capture still on its way is dropped with its popup; a typed note is kept (saved).
    fn popup_closed(&mut self) -> Task<Message> {
        self.confirm_clear = false;
        if std::mem::take(&mut self.capturing) && self.shot == shot::Phase::Busy {
            self.shot = shot::Phase::Idle;
        }
        if matches!(self.shot, shot::Phase::Note { .. }) { self.finish_note(true) } else { Task::none() }
    }

    fn close_popup(&mut self) -> Task<Message> {
        match self.popup.take() {
            Some(popup) => Task::batch([self.popup_closed(), destroy_popup(popup)]),
            None => Task::none(),
        }
    }
}

impl<const WINDOW_MODE: bool> cosmic::Application for Notifier<WINDOW_MODE> {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;
    // Wayland associates the window with its launcher entry and icon by this ID.
    const APP_ID: &'static str = if WINDOW_MODE { "io.github.ubruckhaus.CosmicMaprismaNotifier.Window" } else { APP_ID };

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut app = Self { core, window_mode: WINDOW_MODE, ..Default::default() };
        app.reload(true);
        corners::set_current(&app.settings.corners);
        if app.window_mode {
            app.resize = window_size::Debounce::new(window_start_size());
        }
        let task = if app.window_mode {
            app.set_header_title(tr("Maprisma Notifier"));
            let id = app.core.main_window_id().unwrap();
            // The desktop's theme for the whole window (title bar too), see look.rs.
            Task::batch([app.set_window_title(tr("Maprisma Notifier"), id), corners::set_app_theme(&app.settings.corners)])
        } else {
            Task::none()
        };
        (app, task)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }

    fn on_window_resize(&mut self, id: window::Id, width: f32, height: f32) {
        if self.window_mode && self.core.main_window_id() == Some(id) {
            self.resize.resized(width, height, std::time::Instant::now());
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![cosmic::iced::time::every(Duration::from_secs(2)).map(|_| Message::Tick)];
        if self.window_mode {
            subscriptions.push(cosmic::iced::time::every(Duration::from_millis(150)).map(|_| Message::CheckClose));
            if self.resize.waiting() {
                subscriptions.push(cosmic::iced::time::every(Duration::from_millis(100)).map(|_| Message::SaveSize));
            }
        } else {
            subscriptions.push(rectangle_tracker_subscription(0).map(|update| Message::Rectangle(update.1)));
        }
        // F12 (no modifiers) in the popup or window; Esc closes the innermost thing (esc_step).
        subscriptions.push(cosmic::iced::event::listen_with(|event, status, id| match event {
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                if shot::is_shot_key(&key, modifiers) {
                    Some(Message::ShotKey(id))
                } else if status == cosmic::iced::event::Status::Ignored
                    && matches!(key, cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::Escape))
                {
                    // Not one an open drop-down list took to close itself.
                    Some(Message::Escape(id))
                } else {
                    None
                }
            }
            _ => None,
        }));
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let mut restyle = self.reload(false);
                // The desktop's colours and corners (Maprisma Theme can change them any time).
                restyle |= maprisma_notifier::look::refresh();
                if restyle && self.window_mode {
                    return corners::set_app_theme(&self.settings.corners);
                }
            }
            Message::SetDnd(on) => {
                self.setting_error = settings::set("dnd", Some(on.into())).is_err();
                self.reload(true);
            }
            Message::ToggleDnd => {
                let on = !self.settings.dnd;
                return self.update(Message::SetDnd(on));
            }
            Message::MarkRead(id) => self.changed(history::mark_read(&id)),
            Message::MarkAllRead => self.changed(history::mark_all_read()),
            Message::ClearApp(app) => self.changed(history::clear_app(&app)),
            Message::AskClearAll => {
                self.confirm_clear = true;
                self.language_open = false;
                self.corners_open = false;
            }
            Message::CancelClear => self.confirm_clear = false,
            Message::ClearAll => {
                self.confirm_clear = false;
                self.changed(history::clear_all());
            }
            Message::OpenSettings => {
                open_manager();
                return self.close_popup();
            }
            Message::Language(code) => {
                self.language_error = set_language(&code).is_err();
                self.language_open = false;
            }
            Message::ToggleLanguage => {
                self.language_open = !self.language_open;
                self.corners_open = false;
            }
            Message::ShotKey(id) => {
                let shown = if self.window_mode { self.core.main_window_id() == Some(id) } else { self.popup == Some(id) };
                if self.settings.screenshot_key && shown && self.shot == shot::Phase::Idle {
                    self.shot = shot::Phase::Busy;
                    self.capturing = true;
                    return window::screenshot(id).map(|picture| cosmic::Action::App(Message::Captured(picture)));
                }
            }
            Message::Captured(picture) => {
                if !std::mem::take(&mut self.capturing) {
                    return Task::none();
                }
                let (width, height) = (picture.size.width, picture.size.height);
                return off_thread(move || shot::save_screenshot(SHOT_FOLDER, &picture.rgba, width, height), Message::ShotSaved);
            }
            Message::ShotSaved(result) => {
                self.shot = match result {
                    Ok(saved) => shot::Phase::Note { path: saved.path, shown: saved.shown, text: String::new() },
                    Err(reason) => shot::Phase::Error(trf("The screenshot could not be saved: {0}", &[reason])),
                };
                if matches!(self.shot, shot::Phase::Note { .. }) {
                    return cosmic::widget::text_input::focus(NOTE_ID.clone());
                }
            }
            Message::NoteInput(value) => {
                if let shot::Phase::Note { text, .. } = &mut self.shot {
                    *text = value.chars().take(shot::NOTE_LIMIT).collect();
                }
            }
            Message::NoteDone => return self.finish_note(true),
            Message::NoteSaved(result) => match result {
                Ok(()) => {
                    if self.shot == shot::Phase::Noting {
                        self.shot = shot::Phase::Idle;
                    }
                }
                Err(reason) => self.shot = shot::Phase::Error(trf("The note could not be saved: {0}", &[reason])),
            },
            Message::Escape(id) => {
                let shown = if self.window_mode { self.core.main_window_id() == Some(id) } else { self.popup == Some(id) };
                match esc_step(self.shot.open(), self.confirm_clear, self.language_open || self.corners_open, shown) {
                    EscStep::Note => return self.finish_note(false),
                    EscStep::Confirm => self.confirm_clear = false,
                    EscStep::Menus => {
                        self.language_open = false;
                        self.corners_open = false;
                    }
                    // Like the window's own close (the tray keeps running); the popup like a click on the icon.
                    EscStep::Close if self.window_mode => {
                        self.save_size(true);
                        return cosmic::iced::exit();
                    }
                    EscStep::Close => return self.close_popup(),
                    EscStep::Nothing => {}
                }
            }
            Message::SetShotKey(on) => {
                self.setting_error = write_pref("screenshot_key", Some(on.into())).is_err();
                self.reload(true);
            }
            Message::ToggleCorners => {
                self.corners_open = !self.corners_open;
                self.language_open = false;
            }
            Message::Corners(choice) => {
                let choice = corners::normalize(&choice);
                // "Like the desktop" is the default: no key in settings.json.
                let value = (choice != "desktop").then(|| Value::from(choice));
                self.setting_error = write_pref("corners", value).is_err();
                self.reload(true);
                self.settings.corners = choice.to_string();
                corners::set_current(choice);
                self.corners_open = false;
                if self.window_mode {
                    return corners::set_app_theme(choice);
                }
            }
            Message::CheckClose => {
                if window_mode::CLOSE.load(Ordering::Relaxed) {
                    self.save_size(true);
                    return cosmic::iced::exit();
                }
            }
            Message::SaveSize => self.save_size(false),
            Message::Rectangle(update) => match update {
                RectangleUpdate::Rectangle(r) => self.rectangle = r.1,
                RectangleUpdate::Init(tracker) => self.tracker = Some(tracker),
            },
            Message::CloseRequested(id) => {
                if self.window_mode {
                    self.save_size(true);
                }
                if self.popup == Some(id) {
                    self.popup = None;
                    return self.popup_closed();
                }
            }
            Message::TogglePopup => {
                if self.window_mode {
                    return Task::none();
                }
                if self.popup.is_some() {
                    return self.close_popup();
                }
                self.reload(true);
                let focus = if matches!(self.shot, shot::Phase::Note { .. }) { cosmic::widget::text_input::focus(NOTE_ID.clone()) } else { Task::none() };
                return Task::batch([
                    cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                        |_| Default::default(),
                        |app: &mut Self| {
                            let id = window::Id::unique();
                            app.popup = Some(id);
                            let mut settings = app.core.applet.get_popup_settings(app.core.main_window_id().unwrap(), id, None, None, None);
                            let Rectangle { x, y, width, height } = app.rectangle;
                            settings.positioner.anchor_rect = Rectangle::<i32> {
                                x: x.max(1.0) as i32,
                                y: y.max(1.0) as i32,
                                width: width.max(1.0) as i32,
                                height: height.max(1.0) as i32,
                            };
                            settings.positioner.size = None;
                            settings
                        },
                        None,
                    )),
                    focus,
                ]);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.window_mode {
            return corners::apply_window(&self.settings.corners, self.popup_content());
        }
        let horizontal = matches!(self.core.applet.anchor, PanelAnchor::Top | PanelAnchor::Bottom);
        let padding = self.core.applet.suggested_padding(true).0;
        let button = button::custom(self.panel_content())
            .padding(if horizontal { [0, padding] } else { [padding, 0] })
            .on_press_down(Message::TogglePopup)
            .class(theme::Button::AppletIcon);
        // Middle click: do not disturb on/off.
        let button = mouse_area(button).on_middle_press(Message::ToggleDnd);
        let content: Element<Message> = match self.tracker.as_ref() {
            Some(tracker) => tracker.container(0, button).ignore_bounds(true).into(),
            None => button.into(),
        };
        autosize::autosize(content, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        if self.window_mode {
            return corners::apply_window(&self.settings.corners, self.popup_content());
        }
        corners::apply(&self.settings.corners, self.core.applet.popup_container(self.popup_content()))
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        if self.window_mode { None } else { Some(cosmic::applet::style()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_closes_the_innermost_thing_first() {
        assert_eq!(esc_step(true, true, true, true), EscStep::Note);
        assert_eq!(esc_step(false, true, true, true), EscStep::Confirm);
        assert_eq!(esc_step(false, false, true, true), EscStep::Menus);
        assert_eq!(esc_step(false, false, false, true), EscStep::Close);
        // Not from the window or the open popup (e.g. the panel button): nothing.
        assert_eq!(esc_step(true, true, true, false), EscStep::Nothing);
        assert_eq!(esc_step(false, false, false, false), EscStep::Nothing);
    }

    #[test]
    fn icon_per_state() {
        assert_eq!(bar_icon(false, 0), ICON);
        assert_eq!(bar_icon(false, 3), ICON_UNREAD);
        assert_eq!(bar_icon(true, 3), ICON_DND);
    }

    #[test]
    fn status_texts() {
        maprisma_notifier::paths::test_roots();
        assert_eq!(status_line(0, false), "No unread notifications");
        assert_eq!(status_line(1, false), "1 unread");
        assert_eq!(status_line(4, true), "4 unread · Do not disturb");
    }
}
