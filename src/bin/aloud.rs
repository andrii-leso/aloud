//! The Tauri menubar app: builds the TTS engine and `Player` once at
//! startup, registers the region hotkey and the macOS Service callback
//! (both funnel into `aloud::app::App`, which serializes them onto the
//! one shared `Player`), and exposes tray items for both actions plus
//! Stop.

use aloud::app::actions::Outcome;
use aloud::app::App;
use aloud::capture::macos::ScreenCapture;
use aloud::ocr::macos::VisionOcr;
use aloud::play::player::Player;
use aloud::play::sink::RodioSink;
use aloud::tts::supertonic_engine::SupertonicEngine;
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

/// Matches `aloud-say`'s default (`src/bin/aloud_say.rs`). A voice picker
/// is a Settings-window concern, not built in M3.
const VOICE: &str = "F5";
/// Matches `aloud-say`'s default. A speed control is a Settings-window /
/// tray concern (see the design doc), not built in M3.
const SPEED: f32 = 1.0;
const REGION_SHORTCUT: &str = "CmdOrCtrl+Shift+R";

/// Menubar tray icon, embedded rather than loaded from a path at runtime —
/// a path-based load can resolve differently once bundled inside the
/// `.app` (relative to whatever the process's cwd happens to be at
/// launch) and would fail silently in exactly the place this is hardest
/// to debug (no window, no attached terminal). 44x44px, drawn as a macOS
/// *template* image (pure black shapes, alpha-only anti-aliasing, no
/// colour) — paired with `.icon_as_template(true)` below so macOS
/// recolours it correctly for both light and dark menu bars. See
/// `icons/tray.png` and the Swift script that drew it.
const TRAY_ICON: &[u8] = include_bytes!("../../icons/tray.png");

/// Everything a hotkey handler or tray click needs, built once at startup
/// and shared for the life of the process.
struct Runtime {
    app: App,
    selector: ScreenCapture,
    ocr: VisionOcr,
    tray: TrayIcon<tauri::Wry>,
    status_item: MenuItem<tauri::Wry>,
}

/// The status menu item's text when nothing is wrong.
const STATUS_READY: &str = "Ready";

/// Resets the tray status to `Ready` and the tooltip to plain `Aloud`,
/// clearing any error left over from a previous failed action so it
/// doesn't linger forever once a subsequent action succeeds.
fn reset_status(rt: &Runtime) {
    let _ = rt.status_item.set_text(STATUS_READY);
    let _ = rt.tray.set_tooltip(Some("Aloud"));
}

/// Surfaces a failure in the tray: the disabled status item at the top
/// of the menu, and the tray tooltip. There is no window to show an
/// error in, and this needs no permissions or entitlements — unlike a
/// system notification banner, which macOS attributes to `osascript`'s
/// own identity (Script Editor) rather than Aloud, so it lands in the
/// wrong app's notification settings and can be silently suppressed
/// there without the user ever connecting it to Aloud. This replaced
/// that `osascript`-based approach entirely.
///
/// The status item gets a short form, since a long line in a menu is
/// easy to clip or misread; the tooltip carries the full, actionable
/// text (e.g. the Screen Recording message must keep naming System
/// Settings → Privacy & Security → Screen Recording and the restart
/// requirement — see `src/capture/macos.rs`).
fn set_error_status(rt: &Runtime, message: &str) {
    let short = if message.contains("Screen Recording") {
        "Screen Recording permission needed".to_string()
    } else if message.chars().count() <= 60 {
        message.to_string()
    } else {
        let mut s: String = message.chars().take(57).collect();
        s.push_str("...");
        s
    };
    let _ = rt.status_item.set_text(format!("⚠ {short}"));
    let _ = rt.tray.set_tooltip(Some(format!("Aloud — {message}")));
}

/// Runs `read_region` on a background thread so the caller (the shortcut
/// handler or the tray event handler, both on the main thread) returns
/// immediately. `App::read_region` itself guards against a second call
/// landing while one is already in flight.
///
/// Status policy: a busy skip and a deliberate Escape cancel both leave
/// the tray status untouched — the first because a read is already
/// underway, the second because a cancel is not a failure. A successful
/// read resets the status to `Ready` (clearing any stale error). An
/// empty result (captured something, found no text) and any `Err` (most
/// importantly the missing Screen Recording permission from Task 3,
/// whose message already names System Settings and the required
/// restart — see `src/capture/macos.rs`) are surfaced, since both look
/// identical to "the hotkey did nothing" otherwise.
fn spawn_read_region(rt: Arc<Runtime>) {
    std::thread::spawn(move || match rt.app.read_region(&rt.selector, &rt.ocr) {
        Ok(None) | Ok(Some(Outcome::Cancelled)) => {}
        Ok(Some(Outcome::Spoke)) => reset_status(&rt),
        Ok(Some(Outcome::Empty)) => {
            eprintln!("[aloud] read_region: no text found in the captured region");
            set_error_status(&rt, "No text found in that region.");
        }
        Err(e) => {
            eprintln!("[aloud] read_region failed: {e:#}");
            set_error_status(&rt, &e.to_string());
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

            let status_item = MenuItem::with_id(app, "status", STATUS_READY, false, None::<&str>)?;
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
                &[
                    &status_item,
                    &read_region_item,
                    &read_selection_item,
                    &stop_item,
                    &quit,
                ],
            )?;

            let tray = TrayIconBuilder::new()
                .icon(Image::from_bytes(TRAY_ICON)?)
                .icon_as_template(true)
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

            let runtime = Arc::new(Runtime {
                app: core,
                selector: ScreenCapture::new(),
                ocr: VisionOcr::new()?,
                tray,
                status_item,
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
                aloud::selection::macos::register_service_provider(Arc::new(
                    move |text: String| {
                        let rt = Arc::clone(&rt);
                        std::thread::spawn(move || match rt.app.speak_selection(&text) {
                            Ok(true) => reset_status(&rt),
                            Ok(false) => {}
                            Err(e) => {
                                eprintln!("[aloud] speak_selection failed: {e:#}");
                                set_error_status(&rt, &e.to_string());
                            }
                        });
                    },
                ))?;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Aloud");
}
