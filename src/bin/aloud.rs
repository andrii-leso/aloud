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
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::Manager;
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

/// Matches `aloud-say`'s default (`src/bin/aloud_say.rs`). A voice picker
/// is a Settings-window concern, not built in M3.
const VOICE: &str = "F5";
/// Matches `aloud-say`'s default. A speed control is a Settings-window /
/// tray concern (see the design doc), not built in M3.
const SPEED: f32 = 1.0;

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
        Ok(None) => {
            aloud::log_line!("read_region: skipped, a read is already in flight");
        }
        Ok(Some(Outcome::Cancelled)) => {
            aloud::log_line!("read_region: cancelled by the user (Escape)");
        }
        Ok(Some(Outcome::Spoke)) => {
            aloud::log_line!("read_region: completed, spoke");
            reset_status(&rt);
        }
        Ok(Some(Outcome::Empty)) => {
            eprintln!("[aloud] read_region: no text found in the captured region");
            aloud::log_line!("read_region: completed, no text found in the captured region");
            set_error_status(&rt, "No text found in that region.");
        }
        Err(e) => {
            eprintln!("[aloud] read_region failed: {e:#}");
            aloud::log_line!("read_region: error: {e:#}");
            set_error_status(&rt, &e.to_string());
        }
    });
}

/// Moves the region hotkey from `old` to `new`, rolling back to `old` if
/// the new one will not register.
///
/// Note what this canNOT do: report that a chord is already owned by
/// macOS or another app. Carbon registers non-exclusively, so that case
/// returns Ok here and the hotkey is then silently shadowed. The settings
/// UI confirms liveness by asking the user to press it.
fn apply_shortcut(app: &tauri::AppHandle, old: Option<&str>, new: &str) -> Result<(), String> {
    use aloud::shortcut::Step;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let gs = app.global_shortcut();
    for step in aloud::shortcut::plan_apply(old, new) {
        match step {
            Step::Unregister(s) => {
                // Returns Ok for a chord that was never registered, so a
                // failure here is a real one.
                if let Err(e) = gs.unregister(s.as_str()) {
                    return Err(format!("could not release {s}: {e}"));
                }
            }
            Step::Register(s) => {
                if let Err(e) = gs.register(s.as_str()) {
                    // Put the old chord back so the app is never left
                    // with no working hotkey.
                    if let Some(o) = old {
                        let _ = gs.register(o);
                    }
                    return Err(format!("could not register {s}: {e}"));
                }
            }
        }
    }
    Ok(())
}

/// Stub — a liveness probe is added in Task 8. For now no shortcut press
/// is ever a probe, so every press runs the normal read-region flow.
fn probe_consumed(_: &tauri::AppHandle, _: &Shortcut) -> bool {
    false
}

/// `"CmdOrCtrl+Shift+R"` → `"⌘⇧R"`. Display only — the canonical form
/// stays the plugin's string.
fn pretty_accelerator(accel: &str) -> String {
    let mut out = String::new();
    let mut key = "";
    for part in accel.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "cmdorctrl" | "cmd" | "command" | "super" => out.push('⌘'),
            "control" | "ctrl" => out.push('⌃'),
            "alt" | "option" => out.push('⌥'),
            "shift" => out.push('⇧'),
            _ => key = part,
        }
    }
    out.push_str(key);
    out
}

/// Stub — the real settings window is built in Task 5. This exists now so
/// the tray item is wired and inert rather than missing entirely.
fn open_settings_window(_app: &tauri::AppHandle) {
    aloud::log_line!("settings: window not built yet (Task 5)");
}

fn main() {
    // Must run before anything else that might log: when the app is
    // launched as a bundle via LaunchServices (the only way it works
    // correctly — see the Service registration below), stderr is not
    // attached to anything retrievable, so this file is the only place a
    // failure is diagnosable from. See `src/log.rs`.
    aloud::log::init();
    aloud::log_line!("app start");

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    // ShortcutEvent fires on both press and release; without
                    // this filter every hotkey press runs the flow twice.
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    aloud::log_line!("hotkey: {shortcut:?} pressed");
                    // A liveness probe swallows the press instead of reading
                    // a region.
                    if probe_consumed(app, shortcut) {
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

            let settings = aloud::settings::Settings::load(&app.path().app_config_dir()?);

            // Paid once, here: the Supertonic model load is ~1.4s and must
            // happen at launch, not on the first hotkey press.
            let engine = Arc::new(SupertonicEngine::spawn(VOICE)?);
            aloud::log_line!("engine load complete");
            let sink = Arc::new(RodioSink::new()?);
            let player = Player::new(engine, sink);
            let core = App::new(player, SPEED);

            let status_item = MenuItem::with_id(app, "status", STATUS_READY, false, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;

            // Label carries the live chord, so a rebind is reflected here rather
            // than drifting from a second hard-coded copy of the accelerator.
            let read_region_item = MenuItem::with_id(
                app,
                "read_region",
                format!(
                    "Read Region  ({})",
                    pretty_accelerator(&settings.region_shortcut)
                ),
                true,
                None::<&str>,
            )?;

            // Informational, and deliberately worded as a statement of fact rather
            // than an instruction: ⌘⇧A already works, because Info.plist ships it as
            // the Service's NSKeyEquivalent. The previous label told the user to go
            // and assign it, which was untrue.
            let read_selection_item = MenuItem::with_id(
                app,
                "read_selection_info",
                "Read Selection  (⌘⇧A, or the Services menu)",
                false,
                None::<&str>,
            )?;

            // The selection shortcut is a macOS Service, so it can only be changed
            // in System Settings. This opens the exact pane instead of describing
            // where it is.
            let services_settings_item = MenuItem::with_id(
                app,
                "services_settings",
                "Change Selection Shortcut…",
                true,
                None::<&str>,
            )?;
            let separator2 = PredefinedMenuItem::separator(app)?;

            let settings_item =
                MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let stop_item = MenuItem::with_id(app, "stop", "Stop", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Aloud", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status_item,
                    &separator,
                    &read_region_item,
                    &read_selection_item,
                    &services_settings_item,
                    &separator2,
                    &settings_item,
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
                    } else if event.id() == "services_settings" {
                        // Deep link to Keyboard Shortcuts → Services. The Service's own
                        // ⌘⇧A already works; this is for users who want to change it or
                        // whose ⌘⇧A collides with Chrome or Xcode.
                        let target =
                            "x-apple.systempreferences:com.apple.preference.keyboard?Shortcuts";
                        if let Err(e) = std::process::Command::new("open").arg(target).spawn() {
                            aloud::log_line!(
                                "services_settings: could not open System Settings: {e}"
                            );
                        }
                    } else if event.id() == "settings" {
                        open_settings_window(app);
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
            app.manage(Mutex::new(settings.clone()));

            // Registered here rather than via Builder::with_shortcuts because
            // that path propagates a failure out of plugin setup into
            // .run(), which panics to a stderr nothing reads from a bundled
            // launch. Once the chord is user-supplied, that would turn a bad
            // save into an app that silently refuses to start. Here a
            // failure is logged, surfaced in the tray, and recovered from by
            // falling back to the default.
            {
                let handle = app.handle().clone();
                let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                let wanted = settings.region_shortcut.clone();
                match apply_shortcut(&handle, None, &wanted) {
                    Ok(()) => {
                        aloud::log_line!("hotkey: registered {wanted}");
                    }
                    Err(e) => {
                        aloud::log_line!("hotkey: failed to register {wanted}: {e}");
                        if wanted != aloud::settings::DEFAULT_SHORTCUT {
                            match apply_shortcut(&handle, None, aloud::settings::DEFAULT_SHORTCUT) {
                                Ok(()) => {
                                    aloud::log_line!(
                                        "hotkey: fell back to {}",
                                        aloud::settings::DEFAULT_SHORTCUT
                                    );
                                    set_error_status(
                                        &rt,
                                        "Your shortcut could not be registered; the default is back in use.",
                                    );
                                }
                                Err(e2) => {
                                    aloud::log_line!("hotkey: default also failed: {e2}");
                                    set_error_status(&rt, "No region shortcut could be registered.");
                                }
                            }
                        } else {
                            set_error_status(&rt, "The region shortcut could not be registered.");
                        }
                    }
                }
            }

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
                            Ok(true) => {
                                aloud::log_line!("speak_selection: completed, spoke");
                                reset_status(&rt);
                            }
                            Ok(false) => {
                                aloud::log_line!(
                                    "speak_selection: skipped, a read is already in flight"
                                );
                            }
                            Err(e) => {
                                eprintln!("[aloud] speak_selection failed: {e:#}");
                                aloud::log_line!("speak_selection: error: {e:#}");
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
