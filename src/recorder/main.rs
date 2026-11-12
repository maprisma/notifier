//! maprisma-notifier-recorder: see the library's `recorder` module. Started by the tray or the
//! applet (and ending with them), or by hand for a session without either.

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let desktop = maprisma_notifier::look::desktop();
    let code = runtime.block_on(maprisma_notifier::recorder::run(maprisma_notifier::dnd::target(desktop)));
    std::process::exit(code);
}
