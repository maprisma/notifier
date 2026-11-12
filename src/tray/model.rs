//! The tray's menu and texts, as plain data (tested without a bus).
use maprisma_notifier::i18n::{language_options, tr, trf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Left click: the window opens, or closes when it is open.
    Activate,
    Open,
    ToggleDnd,
    ClearAll,
    Settings,
    Quit,
    Language(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    /// A check mark item: Some(checked).
    pub toggle: Option<bool>,
    pub children: Vec<Item>,
    pub action: Option<Action>,
}

impl Item {
    pub fn find(&self, id: i32) -> Option<&Self> {
        if self.id == id { Some(self) } else { self.children.iter().find_map(|c| c.find(id)) }
    }
    pub fn activate(&self, id: i32) -> Option<Action> {
        self.find(id).filter(|i| i.enabled).and_then(|i| i.action)
    }
}

/// What the icon, tooltip and menu show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub unread: usize,
    /// The user's switch (the menu's check mark).
    pub dnd_switch: bool,
    /// In effect (switch or schedule): the icon.
    pub dnd: bool,
    pub any: bool,
}

pub fn shown() -> Shown {
    use maprisma_notifier::{history, settings};
    let s = settings::load();
    let entries: Vec<history::Entry> = history::load().into_iter().filter(|e| !s.hidden_apps.contains(&e.app)).collect();
    Shown {
        unread: history::unread_count(&entries, &s.muted_apps),
        dnd_switch: s.dnd,
        dnd: settings::effective_dnd(&s, settings::local_minutes()),
        any: !entries.is_empty(),
    }
}

pub const ICON: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-symbolic";
pub const ICON_UNREAD: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-unread-symbolic";
pub const ICON_DND: &str = "io.github.ubruckhaus.CosmicMaprismaNotifier-dnd-symbolic";

pub fn icon(shown: &Shown) -> &'static str {
    if shown.dnd {
        ICON_DND
    } else if shown.unread > 0 {
        ICON_UNREAD
    } else {
        ICON
    }
}

pub fn tooltip(shown: &Shown) -> String {
    let count = match shown.unread {
        0 => tr("No unread notifications"),
        1 => tr("1 unread"),
        n => trf("{0} unread", &[n.to_string()]),
    };
    if shown.dnd { format!("{count}\n{}", tr("Do not disturb")) } else { count }
}

pub fn menu(shown: &Shown) -> Item {
    let item = |id, label: &str, action| Item { id, label: label.into(), enabled: true, separator: false, toggle: None, children: vec![], action };
    let mut root = item(0, "", None);
    let mut dnd = item(2, &tr("Do not disturb"), Some(Action::ToggleDnd));
    dnd.toggle = Some(shown.dnd_switch);
    let mut clear = item(3, &tr("Clear all"), Some(Action::ClearAll));
    clear.enabled = shown.any;
    let mut separator = item(5, "", None);
    separator.separator = true;
    let mut languages = item(6, &tr("Language"), None);
    languages.children = language_options().into_iter().enumerate().map(|(index, (_, label))| item(10 + index as i32, &label, Some(Action::Language(index)))).collect();
    root.children = vec![
        item(1, &tr("Open Maprisma Notifier"), Some(Action::Open)),
        dnd,
        clear,
        item(4, &tr("Notification settings…"), Some(Action::Settings)),
        separator,
        languages,
        item(7, &tr("Quit tray"), Some(Action::Quit)),
    ];
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_actions() {
        maprisma_notifier::paths::test_roots();
        let m = menu(&Shown { any: false, dnd_switch: true, ..Default::default() });
        assert_eq!(m.children.len(), 7);
        assert_eq!(m.find(1).unwrap().label, "Open Maprisma Notifier");
        assert_eq!(m.activate(1), Some(Action::Open));
        assert_eq!(m.activate(2), Some(Action::ToggleDnd));
        assert_eq!(m.find(2).unwrap().toggle, Some(true));
        // Nothing to clear: disabled.
        assert_eq!(m.activate(3), None);
        assert_eq!(menu(&Shown { any: true, ..Default::default() }).activate(3), Some(Action::ClearAll));
        assert_eq!(m.activate(4), Some(Action::Settings));
        assert_eq!(m.activate(7), Some(Action::Quit));
        assert_eq!(m.activate(10), Some(Action::Language(0)));
        assert_eq!(m.find(6).unwrap().children.len(), 9);
        assert_eq!(m.activate(0), None);
        assert_eq!(m.activate(99), None);
    }

    #[test]
    fn icon_and_tooltip() {
        maprisma_notifier::paths::test_roots();
        let mut s = Shown::default();
        assert_eq!((icon(&s), tooltip(&s).as_str()), (ICON, "No unread notifications"));
        s.unread = 1;
        assert_eq!((icon(&s), tooltip(&s).as_str()), (ICON_UNREAD, "1 unread"));
        s.unread = 5;
        s.dnd = true;
        assert_eq!((icon(&s), tooltip(&s).as_str()), (ICON_DND, "5 unread\nDo not disturb"));
    }
}
