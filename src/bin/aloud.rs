//! The Tauri menubar app: builds the TTS engine and `Player` once at
//! startup, registers the region hotkey and the macOS Service callback
//! (both funnel into `aloud::app::App`, which serializes them onto the
//! one shared `Player`), and exposes tray items for both actions plus
//! Stop.

use aloud::app::App;
use aloud::capture::macos::ScreenCapture;
use aloud::ocr::macos::VisionOcr;
use aloud::play::player::Player;
use aloud::play::sink::RodioSink;
use aloud::tts::supertonic_engine::SupertonicEngine;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

/// Matches `aloud-say`'s default (`src/bin/aloud_say.rs`). A voice picker
/// is a Settings-window concern, not built in M3.
const VOICE: &str = "F5";
/// Matches `aloud-say`'s default. A speed control is a Settings-window /
/// tray concern (see the design doc), not built in M3.
const SPEED: f32 = 1.0;
const REGION_SHORTCUT: &str = "CmdOrCtrl+Shift+R";

/// Everything a hotkey handler or tray click needs, built once at startup
/// and shared for the life of the process.
struct Runtime {
    app: App,
    selector: ScreenCapture,
    ocr: VisionOcr,
}

/// Runs `read_region` on a background thread so the caller (the shortcut
/// handler or the tray event handler, both on the main thread) returns
/// immediately. `App::read_region` itself guards against a second call
/// landing while one is already in flight.
fn spawn_read_region(rt: Arc<Runtime>) {
    std::thread::spawn(move || {
        if let Err(e) = rt.app.read_region(&rt.selector, &rt.ocr) {
            eprintln!("[aloud] read_region failed: {e:#}");
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts([REGION_SHORTCUT])
                .expect("REGION_SHORTCUT is a valid shortcut string")
                .with_handler(|app, _shortcut, event| {
                    // ShortcutEvent fires on both press and release; without
                    // this filter every hotkey press runs the flow twice.
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                    spawn_read_region(rt);
                })
                .build(),
        )
        .setup(|app| {
            // Menubar app: no Dock icon, no window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Paid once, here: the Supertonic model load is ~1.4s and must
            // happen at launch, not on the first hotkey press.
            let engine = Arc::new(SupertonicEngine::spawn(VOICE)?);
            let sink = Arc::new(RodioSink::new()?);
            let player = Player::new(engine, sink);
            let core = App::new(player, SPEED);

            let runtime = Arc::new(Runtime {
                app: core,
                selector: ScreenCapture::new(),
                ocr: VisionOcr::new()?,
            });
            app.manage(runtime);

            // The selection path is a macOS Service, not a hotkey we own:
            // the system hands us the user's selected text via
            // Services → Read Aloud. No Accessibility permission, no
            // clipboard. Registration must happen on the main thread,
            // which `setup` is.
            //
            // `register_service_provider`'s own callback already runs on a
            // background thread (see `src/selection/macos.rs`), but this
            // closure spawns its own thread anyway rather than relying on
            // that as an implicit contract: `App::speak_selection` (like
            // `App::read_region`) must never run on the main thread, full
            // stop, regardless of what upstream guarantees.
            #[cfg(target_os = "macos")]
            {
                let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                aloud::selection::macos::register_service_provider(Arc::new(move |text: String| {
                    let rt = Arc::clone(&rt);
                    std::thread::spawn(move || {
                        if let Err(e) = rt.app.speak_selection(&text) {
                            eprintln!("[aloud] speak_selection failed: {e:#}");
                        }
                    });
                }))?;
            }

            let read_region_item =
                MenuItem::with_id(app, "read_region", "Read Region  (⌘⇧R)", true, None::<&str>)?;
            let read_selection_item = MenuItem::with_id(
                app,
                "read_selection_info",
                "Read Selection — assign in System Settings ▸ Keyboard ▸ Services",
                false,
                None::<&str>,
            )?;
            let stop_item = MenuItem::with_id(app, "stop", "Stop", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Aloud", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&read_region_item, &read_selection_item, &stop_item, &quit],
            )?;

            TrayIconBuilder::new()
                .tooltip("Aloud")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    if event.id() == "read_region" {
                        let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                        spawn_read_region(rt);
                    } else if event.id() == "stop" {
                        app.state::<Arc<Runtime>>().app.stop();
                    } else if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Aloud");
}
