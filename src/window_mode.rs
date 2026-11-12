use std::sync::atomic::{AtomicBool, Ordering};
pub static CLOSE: AtomicBool = AtomicBool::new(false);
struct Window;
#[zbus::interface(name = "io.github.ubruckhaus.Notifier.Window")]
impl Window {
    fn close(&self) {
        CLOSE.store(true, Ordering::Relaxed);
    }
}
/// Tells the open window to close (a second `--window` start toggles, as the tray does).
pub async fn close_running() {
    if let Ok(connection) = zbus::Connection::session().await {
        let _ = connection
            .call_method(Some("io.github.ubruckhaus.CosmicMaprismaNotifier.Window"), "/Window", Some("io.github.ubruckhaus.Notifier.Window"), "Close", &())
            .await;
    }
}

pub async fn claim() -> zbus::Result<Option<zbus::Connection>> {
    let connection = zbus::connection::Builder::session()?
        .serve_at("/Window", Window)?
        .build()
        .await?;
    match connection
        .request_name_with_flags(
            "io.github.ubruckhaus.CosmicMaprismaNotifier.Window",
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .await
    {
        Ok(_) => Ok(Some(connection)),
        Err(zbus::Error::NameTaken) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "run under dbus-run-session with --ignored"]
    async fn single_instance_and_close() {
        let first = claim().await.unwrap().unwrap();
        assert!(claim().await.unwrap().is_none());
        let client = zbus::Connection::session().await.unwrap();
        let proxy = zbus::Proxy::new(
            &client,
            "io.github.ubruckhaus.CosmicMaprismaNotifier.Window",
            "/Window",
            "io.github.ubruckhaus.Notifier.Window",
        )
        .await
        .unwrap();
        proxy.call::<_, _, ()>("Close", &()).await.unwrap();
        assert!(CLOSE.load(Ordering::Relaxed));
        CLOSE.store(false, Ordering::Relaxed);
        drop(first);
    }
}
