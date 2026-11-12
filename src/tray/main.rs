//! maprisma-notifier-tray: the SNI tray icon for KDE and Hyprland (Maprisma Shell's tray).
//! Bell / bell with a dot (unread) / bell crossed out (do not disturb), "N unread" as tooltip;
//! left click toggles the window, middle click do not disturb, right click the menu. It keeps
//! the recorder running (maprisma-notifier-recorder ends with it).
mod model;
use maprisma_notifier::i18n::{tr, set_language, language_options};
use model::Action;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use zbus::{
    Connection,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Value as Variant},
};

type Props = HashMap<String, OwnedValue>;
type Layout = (i32, Props, Vec<OwnedValue>);
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
struct State {
    menu: model::Item,
    revision: u32,
    shown: model::Shown,
}
impl State {
    fn status(&self) -> &'static str {
        "Active"
    }
    fn tooltip(&self) -> String {
        model::tooltip(&self.shown)
    }
    fn icon(&self) -> &'static str {
        model::icon(&self.shown)
    }
}
type Shared = Arc<Mutex<State>>;
#[derive(Clone)]
struct Menu {
    state: Shared,
    tx: mpsc::UnboundedSender<Action>,
}
fn owned<T: Into<Variant<'static>>>(value: T) -> OwnedValue {
    OwnedValue::try_from(value.into()).unwrap()
}
fn properties(item: &model::Item, names: &[String]) -> Props {
    let mut props = Props::new();
    props.insert("label".into(), owned(item.label.clone()));
    props.insert("enabled".into(), owned(item.enabled));
    props.insert("visible".into(), owned(true));
    if !item.children.is_empty() {
        props.insert("children-display".into(), owned("submenu"));
    }
    if item.separator {
        props.insert("type".into(), owned("separator"));
    }
    if let Some(checked) = item.toggle {
        props.insert("toggle-type".into(), owned("checkmark"));
        props.insert("toggle-state".into(), owned(checked as i32));
    }
    if !names.is_empty() {
        props.retain(|key, _| names.contains(key));
    }
    props
}
fn layout(item: &model::Item, depth: i32, names: &[String]) -> Layout {
    (
        item.id,
        properties(item, names),
        if depth == 0 {
            vec![]
        } else {
            item.children
                .iter()
                .map(|c| {
                    OwnedValue::try_from(Variant::new(layout(
                        c,
                        if depth < 0 { -1 } else { depth - 1 },
                        names,
                    )))
                    .unwrap()
                })
                .collect()
        },
    )
}
impl Menu {
    async fn dispatch(&self, id: i32, event: &str) -> bool {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        let Some(item) = menu.find(id) else {
            return false;
        };
        if event == "clicked" {
            if let Some(action) = menu.activate(item.id) {
                let _ = self.tx.send(action);
            }
        }
        true
    }
}
#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        vec![]
    }
    async fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        let item = menu
            .find(parent_id)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(tr("Unknown menu ID")))?;
        Ok((s.revision, layout(item, recursion_depth, &property_names)))
    }
    async fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        ids.iter()
            .filter_map(|id| {
                menu.find(*id)
                    .map(|i| (*id, properties(i, &property_names)))
            })
            .collect()
    }
    async fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        menu.find(id)
            .and_then(|i| properties(i, &[]).remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(tr("Unknown menu property")))
    }
    async fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        self.dispatch(id, &event_id).await;
    }
    async fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        let mut errors = vec![];
        for (id, event, _, _) in events {
            if !self.dispatch(id, &event).await {
                errors.push(id);
            }
        }
        errors
    }
    async fn about_to_show(&self, _id: i32) -> bool {
        false
    }
    async fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        (
            vec![],
            ids.into_iter()
                .filter(|id| menu.find(*id).is_none())
                .collect(),
        )
    }
    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}
struct Tray {
    state: Shared,
    tx: mpsc::UnboundedSender<Action>,
}
#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Tray {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "maprisma-notifier"
    }
    #[zbus(property)]
    fn title(&self) -> String {
        tr("Maprisma Notifier")
    }
    #[zbus(property)]
    async fn status(&self) -> &str {
        self.state.lock().await.status()
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    async fn icon_name(&self) -> &str {
        self.state.lock().await.icon()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        model::ICON_UNREAD
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    async fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let s = self.state.lock().await;
        (
            s.icon().into(),
            vec![],
            tr("Maprisma Notifier"),
            s.tooltip(),
        )
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap()
    }
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.tx.send(Action::Activate);
    }
    // Middle click.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.tx.send(Action::ToggleDnd);
    }
    fn context_menu(&self, _x: i32, _y: i32) {} // The host displays the exported Menu.
    fn scroll(&self, _delta: i32, _orientation: String) {}
    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
/// A window this tray started is still running (it may not have claimed its name yet).
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The open window's connection. A window this tray just started gets up to 2 s to claim its
/// name, so a second click while it is starting closes it instead of being lost.
async fn window_owner(dbus: &zbus::fdo::DBusProxy<'_>) -> Option<zbus::names::OwnedUniqueName> {
    for _ in 0..40 {
        if let Ok(owner) = dbus
            .get_name_owner("io.github.ubruckhaus.CosmicMaprismaNotifier.Window".try_into().unwrap())
            .await
        {
            return Some(owner);
        }
        if !STARTED.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}
async fn window_action(toggle: bool, connection: &Connection) {
    // The window's own connection (its unique name), so a close never reaches
    // a window that took the well-known name meanwhile; no service activation.
    if let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await {
        if let Some(owner) = window_owner(&dbus).await {
            if toggle {
                if let Ok(p) = zbus::Proxy::new(connection, owner.as_str(), "/Window", "io.github.ubruckhaus.Notifier.Window").await {
                    let _ = p.call::<_, _, ()>("Close", &()).await;
                }
            }
            return;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(mut child) =
            std::process::Command::new(exe.with_file_name("maprisma-notifier"))
                .arg("--window")
                .spawn()
        {
            STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
            std::thread::spawn(move || {
                let _ = child.wait();
                STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
            });
        }
    }
}
/// Shows the new state: icon, tooltip and the menu (its check mark, Clear all) when changed.
async fn refresh(state: &Shared, connection: &Connection, force_menu: bool) -> zbus::Result<()> {
    let shown = model::shown();
    let mut s = state.lock().await;
    let menu = model::menu(&shown);
    if force_menu || s.menu != menu {
        s.menu = menu;
        s.revision += 1;
        Menu::layout_updated(&SignalEmitter::new(connection, "/MenuBar")?, s.revision, 0).await?;
    }
    if s.shown != shown {
        let icon = model::icon(&s.shown) != model::icon(&shown);
        s.shown = shown;
        drop(s);
        let emitter = SignalEmitter::new(connection, "/StatusNotifierItem")?;
        if icon {
            Tray::new_icon(&emitter).await?;
        }
        Tray::new_tool_tip(&emitter).await?;
    }
    Ok(())
}
fn open_manager() {
    if let Ok(mut child) = std::process::Command::new("maprisma-manager").args(["--page", "notifications"]).stdin(std::process::Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let shown = model::shown();
    let initial = State {
        menu: model::menu(&shown),
        revision: 1,
        shown,
    };
    let state = Arc::new(Mutex::new(initial));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let connection = zbus::connection::Builder::session()?
        .serve_at(
            "/StatusNotifierItem",
            Tray {
                state: state.clone(),
                tx: tx.clone(),
            },
        )?
        .serve_at(
            "/MenuBar",
            Menu {
                state: state.clone(),
                tx,
            },
        )?
        .build()
        .await?;
    // One tray per session: a second one ends here (zbus would otherwise
    // queue the name request and keep running).
    match connection
        .request_name_with_flags(
            "io.github.ubruckhaus.CosmicMaprismaNotifier.Tray",
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .await
    {
        Ok(_) => (),
        Err(zbus::Error::NameTaken) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    // This tray's recorder (history, do not disturb); it ends with the tray.
    maprisma_notifier::recorder::supervise();
    // Checking the unique owner also handles Plasma starting after us and watcher restarts.
    let watcher_connection = connection.clone();
    tokio::spawn(async move {
        let mut registered = String::new();
        loop {
            let dbus = zbus::fdo::DBusProxy::new(&watcher_connection).await;
            let owner = match dbus {
                Ok(p) => p
                    .get_name_owner("org.kde.StatusNotifierWatcher".try_into().unwrap())
                    .await
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            if owner.is_empty() {
                registered.clear();
            } else if owner != registered {
                if let Ok(proxy) = zbus::Proxy::new(
                    &watcher_connection,
                    owner.as_str(),
                    "/StatusNotifierWatcher",
                    "org.kde.StatusNotifierWatcher",
                )
                .await
                {
                    if proxy
                        .call::<_, _, ()>(
                            "RegisterStatusNotifierItem",
                            &(watcher_connection.unique_name().unwrap().as_str(),),
                        )
                        .await
                        .is_ok()
                    {
                        registered = owner;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tick.tick() => refresh(&state, &connection, false).await?,
            Some(action) = rx.recv() => {
                match action {
                    Action::Language(index) => {
                        if let Some((code, _)) = language_options().get(index) {
                            let _ = set_language(code);
                        }
                        refresh(&state, &connection, true).await?;
                    }
                    Action::ToggleDnd => {
                        let on = !maprisma_notifier::settings::load().dnd;
                        let _ = maprisma_notifier::settings::set("dnd", Some(on.into()));
                        refresh(&state, &connection, false).await?;
                    }
                    Action::ClearAll => {
                        let _ = maprisma_notifier::history::clear_all();
                        refresh(&state, &connection, false).await?;
                    }
                    Action::Settings => open_manager(),
                    Action::Quit => return Ok(()),
                    Action::Open => window_action(false, &connection).await,
                    Action::Activate => window_action(true, &connection).await,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dbus_layout_signature_and_depth() {
        maprisma_notifier::paths::test_roots();
        let menu = model::menu(&model::Shown::default());
        assert_eq!(
            Variant::new(layout(&menu, -1, &[]))
                .value_signature()
                .to_string(),
            "(ia{sv}av)"
        );
        assert!(layout(&menu, 0, &[]).2.is_empty());
        assert_eq!(layout(&menu, 1, &[]).2.len(), 7);
        let dnd = properties(menu.find(2).unwrap(), &[]);
        assert_eq!(String::try_from(dnd["toggle-type"].try_clone().unwrap()).unwrap(), "checkmark");
        assert_eq!(i32::try_from(dnd["toggle-state"].try_clone().unwrap()).unwrap(), 0);
        assert_eq!(
            properties(menu.find(1).unwrap(), &["label".into()]).len(),
            1
        );
    }
    #[tokio::test]
    async fn menu_events() {
        maprisma_notifier::paths::test_roots();
        let state = Arc::new(Mutex::new(State {
            menu: model::menu(&model::Shown { any: true, ..Default::default() }),
            revision: 1,
            shown: model::Shown::default(),
        }));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let menu = Menu { state, tx };
        menu.event(1, "clicked".into(), owned(0i32), 0).await;
        assert_eq!(rx.try_recv().unwrap(), Action::Open);
        menu.event(2, "clicked".into(), owned(0i32), 0).await;
        assert_eq!(rx.try_recv().unwrap(), Action::ToggleDnd);
        menu.event(2, "hovered".into(), owned(0i32), 0).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(
            menu.event_group(vec![
                (99, "clicked".into(), owned(0i32), 0),
                (7, "clicked".into(), owned(0i32), 0)
            ])
            .await,
            vec![99]
        );
        assert_eq!(rx.try_recv().unwrap(), Action::Quit);
    }
}
