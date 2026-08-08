use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Menubar app: no Dock icon, no window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // The selection path is a macOS Service, not a hotkey we own:
            // the system hands us the user's selected text via
            // Services → Read Aloud. No Accessibility permission, no
            // clipboard. Registration must happen on the main thread, which
            // `setup` is.
            #[cfg(target_os = "macos")]
            aloud::selection::macos::register_service_provider(std::sync::Arc::new(
                |text: String| {
                    // Placeholder until Task 5 builds the speaking pipeline
                    // and swaps in `read_selection`. Logs the length only —
                    // a selection can be anything, so the text itself must
                    // never reach a log.
                    eprintln!(
                        "[aloud] Service delivered a selection of {} characters",
                        text.chars().count()
                    );
                },
            ))?;

            let quit = MenuItem::with_id(app, "quit", "Quit Aloud", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            TrayIconBuilder::new()
                .tooltip("Aloud")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Aloud");
}
