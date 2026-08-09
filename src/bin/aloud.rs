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
use aloud::play::sink::{AudioSink, RodioSink};
use aloud::settings::{Settings, VOICES};
use aloud::shortcut::Chord;
use aloud::tts::supertonic_engine::SupertonicEngine;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, State};
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
    /// Handle to the tray's "Read Region" item, so `refresh_tray_labels`
    /// can update its label after a rebind without rebuilding the menu.
    read_region_item: MenuItem<tauri::Wry>,
    /// Retained so a live voice swap (`spawn_voice_swap`) can build a new
    /// `Player` around the same audio output rather than opening a second
    /// device — only the engine changes.
    sink: Arc<dyn AudioSink>,
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
///
/// If registering `new` fails, the rollback to `old` is attempted and its
/// own outcome is checked, not discarded: if the rollback also fails, the
/// error says plainly that there is currently no region shortcut at all,
/// rather than reusing the ordinary "new chord rejected" message — that
/// message would read as if `old` were still working when it is not.
/// `set_shortcut` is the first caller where `old` is ever `Some`, so this
/// path was unreachable before Task 6.
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
                    if let Some(o) = old {
                        if gs.register(o).is_err() {
                            // Both the new chord and the rollback failed:
                            // the app now has NO region hotkey. Say so,
                            // rather than reporting the ordinary "new
                            // chord rejected" case.
                            return Err(format!(
                                "could not register {s}, and restoring {o} also failed - \
                                 there is currently no region shortcut. Open Settings and pick one."
                            ));
                        }
                    }
                    return Err(format!("could not register {s}: {e}"));
                }
            }
        }
    }
    Ok(())
}

/// True while the settings window is waiting for the user to press the
/// newly-bound chord as proof it actually works. macOS registers Carbon
/// hotkeys non-exclusively: `register()` returns `Ok(())` for a chord
/// already owned by macOS or another app, and the event is then silently
/// shadowed — there is no API that reports that contention. A press
/// arriving while this flag is set is the only honest confirmation
/// available, so it is consumed as proof-of-life instead of starting a
/// region capture (otherwise confirming your shortcut would fire a
/// crosshair at you).
static PROBE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The probe's actual state transition: arm-once, consume-once. Pulled out
/// of `probe_consumed` so the swap-and-check logic — the part that makes a
/// second, unrelated press unable to falsely "confirm" a stale probe — has
/// a unit test that does not need a `tauri::AppHandle` to construct.
fn take_probe(flag: &AtomicBool) -> bool {
    flag.swap(false, Ordering::SeqCst)
}

/// Clears the probe flag and logs why. Shared by the `end_probe` IPC
/// command — called by the settings page on a 10s timeout and when the
/// user starts recording a different chord — and the settings-window
/// close handlers below, which cannot rely on the page's own JS running
/// to make that same call once its webview has already been torn down.
fn disarm_probe(reason: &str) {
    PROBE_ACTIVE.store(false, Ordering::SeqCst);
    aloud::log_line!("probe: disarmed ({reason})");
}

/// Consumes a hotkey press as a liveness confirmation if the settings page
/// just armed one, instead of letting it fall through to the normal
/// read-region flow. See `PROBE_ACTIVE` for why this exists.
fn probe_consumed(app: &tauri::AppHandle, _shortcut: &Shortcut) -> bool {
    if !take_probe(&PROBE_ACTIVE) {
        return false;
    }
    aloud::log_line!("hotkey: probe confirmed");
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.emit("aloud://probe-fired", ());
    }
    true
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

/// Everything the settings page needs on load.
#[derive(serde::Serialize)]
struct SettingsView {
    region_shortcut: String,
    region_shortcut_pretty: String,
    voice: String,
    speed: f32,
}

/// `Settings` behind a `Mutex`, plus the config directory it was loaded
/// from and is saved back to — set up once in `setup` from the one
/// `Settings::load` call already made there, never loaded a second time.
struct SettingsState {
    settings: Mutex<Settings>,
    config_dir: std::path::PathBuf,
}

// Every `#[tauri::command]` fn below is intentionally NOT `pub`: the macro
// makes its hidden `__cmd__*`/`__tauri_command_name_*` helper macros
// `#[macro_export]` whenever the function is `pub` (so they can be
// reached through a path, e.g. `commands::get_settings`, from a separate
// module). `#[macro_export]` hoists a macro to the crate root — but this
// file already IS the crate root of the `aloud` binary, so a `pub fn`
// here makes the hoisted copy collide with the original in the same
// scope (rustc E0255, "defined multiple times" / "reimported here").
// Plain (crate-private) visibility is correct once `generate_handler!`
// lives in this same file, and matches every other fn in it.
#[tauri::command]
fn get_settings(state: State<'_, SettingsState>) -> SettingsView {
    let s = state.settings.lock().unwrap();
    SettingsView {
        region_shortcut: s.region_shortcut.clone(),
        region_shortcut_pretty: pretty_accelerator(&s.region_shortcut),
        voice: s.voice.clone(),
        speed: s.speed,
    }
}

/// Validates the recorded chord, applies it to the OS, then persists.
///
/// Order matters: nothing is written to disk until the OS accepted the
/// chord, so a rejected chord cannot come back after a restart.
#[tauri::command]
fn set_shortcut(
    app: AppHandle,
    state: State<'_, SettingsState>,
    chord: Chord,
) -> Result<String, String> {
    let accel = chord.to_accelerator().map_err(|e| e.to_string())?;

    let old = { state.settings.lock().unwrap().region_shortcut.clone() };
    apply_shortcut(&app, Some(&old), &accel)?;

    {
        let mut s = state.settings.lock().unwrap();
        s.region_shortcut = accel.clone();
        s.save(&state.config_dir).map_err(|e| e.to_string())?;
    } // guard dropped here — refresh_tray_labels below takes a different
      // lock (Runtime, not SettingsState), so this was never a deadlock,
      // but there is no reason to hold the settings lock across an OS
      // (MenuItem::set_text) call either.
    aloud::log_line!("settings: region shortcut is now {accel}");

    refresh_tray_labels(&app, &accel);
    Ok(pretty_accelerator(&accel))
}

/// Keeps the tray's "Read Region" label in sync with the registered
/// chord, so neither a rebind made in Settings nor a fallback at launch
/// (see the `setup()` hotkey-registration block below) can leave the
/// tray showing a chord that is not actually registered.
fn refresh_tray_labels(app: &tauri::AppHandle, region_shortcut: &str) {
    let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
    let _ = rt.read_region_item.set_text(format!(
        "Read Region  ({})",
        pretty_accelerator(region_shortcut)
    ));
}

#[tauri::command]
fn set_voice(app: AppHandle, state: State<'_, SettingsState>, voice: String) -> Result<(), String> {
    if !VOICES.contains(&voice.as_str()) {
        return Err(format!("unknown voice {voice}"));
    }
    {
        let mut s = state.settings.lock().unwrap();
        s.voice = voice.clone();
        s.save(&state.config_dir).map_err(|e| e.to_string())?;
    }
    spawn_voice_swap(&app, voice);
    Ok(())
}

/// Rebuilds the TTS engine on the requested voice and swaps it in.
///
/// Off the main thread: `SupertonicEngine::spawn` loads the voice style
/// and stands up a worker thread, which takes on the order of a second —
/// running it inline on the IPC call would freeze the settings page's
/// event loop for that long. The settings page shows "Switching voice…"
/// until this reports back.
///
/// Reuses `rt.sink`: only the engine differs between voices, and the
/// swap must not open a second audio device (or worse, leave the old one
/// dangling — `RodioInner::open`'s doc notes playback stops when its
/// device handle is dropped).
fn spawn_voice_swap(app: &tauri::AppHandle, voice: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
        match SupertonicEngine::spawn(&voice) {
            Ok(engine) => {
                let player = Player::new(Arc::new(engine), Arc::clone(&rt.sink));
                // Blocks until any speak() currently holding the read
                // lock finishes — see the RwLock doc comment on
                // `App::swap_player` (src/app/mod.rs).
                rt.app.swap_player(player);
                aloud::log_line!("voice: switched to {voice}");
            }
            Err(e) => {
                aloud::log_line!("voice: could not switch to {voice}: {e:#}");
                set_error_status(&rt, &format!("Could not load the {voice} voice."));
            }
        }
    });
}

#[tauri::command]
fn set_speed(app: AppHandle, state: State<'_, SettingsState>, speed: f32) -> Result<f32, String> {
    let clamped = Settings::clamp_speed(speed);
    {
        let mut s = state.settings.lock().unwrap();
        s.speed = clamped;
        s.save(&state.config_dir).map_err(|e| e.to_string())?;
    }
    set_live_speed(&app, clamped);
    Ok(clamped)
}

/// Applies the new speed to the live `Player` immediately. No engine
/// rebuild, no restart: `speed` is a plain per-call argument to
/// `Player::speak` (unlike voice, which is baked into the engine at
/// construction), so `App` just needs the new value on hand for the next
/// `speak()` call — see `App::set_speed` (src/app/mod.rs).
fn set_live_speed(app: &tauri::AppHandle, speed: f32) {
    app.state::<Arc<Runtime>>().app.set_speed(speed);
    aloud::log_line!("speed: now {speed}");
}

/// Arms the liveness probe: the next real hotkey press is consumed as a
/// confirmation instead of starting a region capture. Called by the
/// settings page right after a rebind succeeds.
#[tauri::command]
fn begin_probe() {
    PROBE_ACTIVE.store(true, Ordering::SeqCst);
    aloud::log_line!("probe: armed");
}

/// Disarms the liveness probe without waiting for a press. Called by the
/// settings page on its 10s timeout and when the user starts recording a
/// different chord before confirming the current one.
#[tauri::command]
fn end_probe() {
    disarm_probe("page request");
}

/// Deep link to Keyboard Shortcuts → Services, shared by the tray menu's
/// "Change Selection Shortcut…" item and the `open_services_settings` IPC
/// command below — one process, one failure path.
fn open_system_shortcuts_pane() -> std::io::Result<std::process::Child> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.keyboard?Shortcuts")
        .spawn()
}

#[tauri::command]
fn open_services_settings() {
    if let Err(e) = open_system_shortcuts_pane() {
        aloud::log_line!("open_services_settings: {e}");
    }
}

/// Shows the settings window, creating it on first use.
///
/// `show()` MUST precede `set_focus()`: tao's set_focus early-returns on
/// a non-visible window and reports nothing. And the window must keep its
/// decorations — canBecomeKey is false without a title bar, which would
/// leave the chord recorder unable to receive a single keystroke.
///
/// Because the app runs `ActivationPolicy::Accessory` (no Dock icon),
/// activation on macOS 14+ is a *request* the system may decline
/// (tauri#6781 is a live report of exactly this failing for a tray-shown
/// text input) — accessory apps do not activate implicitly, so without
/// flipping to `Regular` while the window is open, it can appear behind
/// the frontmost app. `Accessory` is restored on close so no Dock icon
/// lingers.
fn open_settings_window(app: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(w) = app.get_webview_window("settings") {
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = w.show();
        let _ = w.set_focus();
        aloud::log_line!("settings: re-showed existing window");
        return;
    }

    // Declared in tauri.conf.json with visible:false, so it exists but is
    // hidden; this branch only runs if it was closed and destroyed.
    match tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Aloud Settings")
    .inner_size(480.0, 560.0)
    .resizable(false)
    .decorations(true)
    .center()
    .build()
    {
        Ok(w) => {
            w.on_window_event({
                let app = app.clone();
                move |e| {
                    if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
                        // Accessory apps do not activate implicitly; without
                        // this the window can appear behind the frontmost
                        // app the next time it's opened. Restored on close
                        // so no Dock icon lingers.
                        #[cfg(target_os = "macos")]
                        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                        // A pending probe belongs to this webview's JS,
                        // which is about to be destroyed along with the
                        // window — its 10s timeout will never fire to
                        // clear PROBE_ACTIVE on its own, so a forgotten
                        // window would otherwise leave a real hotkey press
                        // silently swallowed forever.
                        disarm_probe("settings window closed");
                    }
                }
            });
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let _ = w.show();
            let _ = w.set_focus();
            aloud::log_line!("settings: created window");
        }
        Err(e) => aloud::log_line!("settings: could not create window: {e}"),
    }
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
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_shortcut,
            set_voice,
            set_speed,
            open_services_settings,
            begin_probe,
            end_probe,
        ])
        .setup(|app| {
            // Menubar app: no Dock icon, no window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Loaded once, here, and wired into `SettingsState` below —
            // never loaded a second time.
            let config_dir = app.path().app_config_dir()?;
            let settings = Settings::load(&config_dir);

            // Paid once, here: the Supertonic model load is ~1.4s and must
            // happen at launch, not on the first hotkey press.
            let engine = Arc::new(SupertonicEngine::spawn(VOICE)?);
            aloud::log_line!("engine load complete");
            // Typed as the trait object up front so the same handle can be
            // both handed to `Player::new` and retained on `Runtime` — a
            // live voice swap (`spawn_voice_swap`) reuses it rather than
            // opening a second audio device.
            let sink: Arc<dyn AudioSink> = Arc::new(RodioSink::new()?);
            let player = Player::new(engine, Arc::clone(&sink));
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
                        if let Err(e) = open_system_shortcuts_pane() {
                            aloud::log_line!(
                                "services_settings: could not open System Settings: {e}"
                            );
                            let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                            set_error_status(&rt, &format!("Could not open System Settings: {e}"));
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
                read_region_item,
                sink,
            });
            app.manage(runtime);
            app.manage(SettingsState {
                settings: Mutex::new(settings.clone()),
                config_dir,
            });

            // The settings window is declared in tauri.conf.json (visible:
            // false), so it already exists at this point — every ordinary
            // open goes through `open_settings_window`'s re-show branch,
            // never its window-builder branch. The close handler that
            // restores ActivationPolicy::Accessory has to be attached here,
            // to this config-created window, or it would never run in
            // practice.
            if let Some(w) = app.get_webview_window("settings") {
                let app_handle = app.handle().clone();
                w.on_window_event(move |e| {
                    if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
                        // Accessory apps do not activate implicitly; without
                        // this the window can appear behind the frontmost
                        // app the next time it's opened.
                        #[cfg(target_os = "macos")]
                        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
                        // See the matching comment in open_settings_window's
                        // builder branch: this window's JS is torn down on
                        // close, so its 10s probe timeout can never run —
                        // clear PROBE_ACTIVE here instead.
                        disarm_probe("settings window closed");
                    }
                });
            }

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
                                    // In-memory only — deliberately NOT
                                    // persisted. The OS now has the
                                    // default registered, not `wanted`,
                                    // so the tray label (built above from
                                    // the pre-fallback `settings`) and
                                    // the in-state `Settings` both need
                                    // to agree with reality, the same way
                                    // `set_shortcut` keeps them in sync
                                    // for an interactive rebind. But this
                                    // failure is presumed transient (e.g.
                                    // another app briefly holding the
                                    // same chord) rather than a permanent
                                    // rejection — `set_shortcut` already
                                    // screens out permanent rejections
                                    // before anything is ever saved — so
                                    // `wanted` must survive on disk for a
                                    // later launch to retry it, not be
                                    // overwritten by the fallback.
                                    {
                                        let settings_state = app.state::<SettingsState>();
                                        let mut guard =
                                            settings_state.settings.lock().unwrap();
                                        guard.region_shortcut =
                                            aloud::settings::DEFAULT_SHORTCUT.to_string();
                                    }
                                    refresh_tray_labels(
                                        &handle,
                                        aloud::settings::DEFAULT_SHORTCUT,
                                    );
                                    set_error_status(
                                        &rt,
                                        "Your shortcut could not be registered; the default is back in use.",
                                    );
                                }
                                Err(e2) => {
                                    aloud::log_line!("hotkey: default also failed: {e2}");
                                    // Neither `wanted` nor the default
                                    // registered: there is no region
                                    // shortcut active at all, so the
                                    // tray's pre-fallback label (built
                                    // above from the persisted, still-
                                    // unregistered `wanted`) would
                                    // otherwise keep telling the user to
                                    // press a chord that does nothing —
                                    // the sibling case Task 6 fixed just
                                    // above, but this branch was missed.
                                    // An empty accelerator renders as an
                                    // empty pretty string (see
                                    // `pretty_accelerator`'s own tests),
                                    // which is honest here: there is
                                    // nothing to show. The status/tooltip
                                    // set below carries the explanation.
                                    refresh_tray_labels(&handle, "");
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

/// Characterisation tests for `pretty_accelerator`. Its inverse,
/// `Chord::to_accelerator` (`src/shortcut.rs`), already has a full suite in
/// `tests/shortcut.rs`; this covers the same class of case for the display
/// side, since Task 6 extends `pretty_accelerator` to render arbitrary
/// user-entered chords and a careless edit here would otherwise go
/// uncaught until someone noticed a wrong glyph on screen.
#[cfg(test)]
mod tests {
    use super::pretty_accelerator;

    #[test]
    fn default_region_shortcut_renders_as_the_shipped_menu_string() {
        // Matches `settings::DEFAULT_SHORTCUT` and the exact string in the
        // shipped tray menu's Read Region item.
        assert_eq!(pretty_accelerator("CmdOrCtrl+Shift+R"), "⌘⇧R");
    }

    #[test]
    fn all_four_modifiers_render_in_the_order_to_accelerator_emits_them() {
        // `Chord::to_accelerator` always builds accelerators in this fixed
        // order (CmdOrCtrl, Alt, Control, Shift, then key — see
        // `src/shortcut.rs`), and `pretty_accelerator` just walks the
        // string left to right, so the glyphs must come out in the same
        // order. A careless edit (e.g. reordering the match arms and
        // assuming that's cosmetic) would scramble this silently.
        assert_eq!(pretty_accelerator("CmdOrCtrl+Alt+Control+Shift+R"), "⌘⌥⌃⇧R");
    }

    #[test]
    fn multi_character_key_tokens_keep_their_original_casing() {
        assert_eq!(pretty_accelerator("CmdOrCtrl+Shift+ArrowUp"), "⌘⇧ArrowUp");
        assert_eq!(pretty_accelerator("Alt+F7"), "⌥F7");
    }

    #[test]
    fn modifier_matching_is_case_insensitive_but_the_key_is_passed_through_as_is() {
        // Only the modifier tokens are lowercased before matching; the key
        // token is never touched, so it comes out exactly as given —
        // here that happens to be lowercase because the input was.
        assert_eq!(pretty_accelerator("cmdorctrl+shift+r"), "⌘⇧r");
    }

    // `pretty_accelerator` is display-only — the canonical form used for
    // actual registration is always the plugin's own accelerator string,
    // produced by `Chord::to_accelerator`, which never emits any of the
    // malformed shapes below. These three exist purely so a future edit
    // cannot reintroduce a panic (e.g. an unchecked index or unwrap) on
    // input this function was never guaranteed well-formed input to begin
    // with. The accepted behaviour for malformed input is: modifiers
    // found are still rendered, and a missing/empty key renders as
    // nothing (not a placeholder, not an error) — asserted explicitly
    // below rather than merely checking these don't panic.
    #[test]
    fn empty_input_yields_an_empty_string() {
        assert_eq!(pretty_accelerator(""), "");
    }

    #[test]
    fn trailing_separator_with_no_key_drops_the_key_silently() {
        assert_eq!(pretty_accelerator("CmdOrCtrl+"), "⌘");
    }

    #[test]
    fn modifiers_only_with_no_key_renders_just_the_modifiers() {
        assert_eq!(pretty_accelerator("Shift"), "⇧");
    }
}

/// Unit tests for `take_probe`, the swap-and-check that decides whether a
/// hotkey press is a liveness confirmation. Each test uses its own local
/// `AtomicBool` rather than the process-wide `PROBE_ACTIVE`, since the
/// real static is shared with every other test in this binary (including
/// the IPC tests below, which run concurrently by default) and would make
/// these flaky/order-dependent for no reason — the function under test
/// takes the flag as a parameter for exactly this reason.
#[cfg(test)]
mod probe_tests {
    use super::take_probe;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn disarmed_flag_is_never_taken() {
        let flag = AtomicBool::new(false);
        assert!(!take_probe(&flag));
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn armed_flag_is_taken_exactly_once() {
        // This is the property that makes double-consumption impossible:
        // an OS key-repeat or a second stray press arriving right after a
        // real confirmation must not be able to "confirm" a second time
        // off the same arm, because there is nothing left to take.
        let flag = AtomicBool::new(true);
        assert!(take_probe(&flag), "the first press must consume the arm");
        assert!(
            !take_probe(&flag),
            "a second press must find nothing left to consume"
        );
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn re_arming_after_a_take_works_again() {
        // Mirrors begin_probe -> (confirmed) -> begin_probe for a second
        // rebind attempt: taking the flag must not leave it permanently
        // unusable.
        let flag = AtomicBool::new(true);
        assert!(take_probe(&flag));
        flag.store(true, Ordering::SeqCst);
        assert!(take_probe(&flag));
    }
}

/// IPC-level test for `get_settings`, the one Task 6 command reachable
/// through `tauri::test`'s `MockRuntime`.
///
/// The settings page's JS does not exist yet (Task 7), so there is no
/// scripted caller to drive through a real webview; and the manual
/// alternative — opening the tray menu and typing into the settings
/// window's devtools console — needs a real click on the menu-bar icon,
/// which this suite (like any non-interactive check) cannot do without
/// synthetic input. `MockRuntime` is the documented way around that: it
/// runs the *actual* invoke pipeline (command-name resolution, argument
/// deserialization, the command body, response serialization)
/// headlessly.
///
/// This uses `mock_context(noop_assets())`, not this crate's real
/// `tauri.conf.json`/`capabilities/` via `generate_context!()`: that
/// macro embeds a process-wide symbol (`_EMBED_INFO_PLIST`) that can only
/// be defined once per binary, and `main()` below already defines it —
/// calling it again here, even from code `main()` never executes, fails
/// the whole test binary at link time with "symbol ... already defined"
/// (confirmed empirically). So this test proves dispatch and
/// serialization are wired correctly; it does NOT exercise this crate's
/// actual ACL/capabilities (`capabilities/default.json`) — see the Task
/// 6 report for how that gap was covered instead.
///
/// `set_shortcut`, `set_voice`, and `set_speed` cannot be reached this
/// way at all: they take `app: AppHandle`, which — unqualified — means
/// `AppHandle<Wry>` (Tauri's default runtime), matching every other
/// `AppHandle` in this file (`apply_shortcut`, `open_settings_window`,
/// `refresh_tray_labels`, ...). `generate_handler!` requires every
/// registered command to satisfy `CommandArg<'_, R>` for the builder's
/// own `R`, so pairing them with `mock_builder()`'s `R = MockRuntime`
/// fails at compile time (confirmed: `error[E0277]: the trait bound
/// `AppHandle: CommandArg<'_, MockRuntime>` is not satisfied`) —
/// `get_settings` alone has no such parameter, so it is the only one
/// that can be registered against `MockRuntime`. Making the other three
/// generic over `R: tauri::Runtime` would fix this, but would mean
/// deviating from the concrete-`Wry` style used everywhere else in this
/// file for a testing convenience the production app never needs; the
/// Task 6 report explains what verified those three instead (primarily
/// code-reading, since their validation/ordering is what the CARRIED
/// FORWARD fix and the disk-write ordering both depend on).
#[cfg(test)]
mod command_tests {
    use super::*;
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    fn invoke_url() -> tauri::Url {
        // Matches the URL scheme the real webview's IPC bridge uses per
        // platform (see the `tauri::test` doctests this is copied from).
        if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .unwrap()
    }

    fn request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: invoke_url(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        }
    }

    #[test]
    fn get_settings_returns_the_expected_shape() {
        // A throwaway config dir under the OS temp dir — never the real
        // `~/Library/Application Support/com.andriileso.aloud/`, so this
        // test can never touch the owner's actual `settings.json`.
        let dir = std::env::temp_dir().join(format!(
            "aloud-command-tests-get-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![get_settings])
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(SettingsState {
            settings: Mutex::new(Settings::default()),
            config_dir: dir.clone(),
        });
        // The noop context declares no windows (see the doc comment
        // above for why it's noop rather than this crate's real
        // context), so build one ad hoc rather than fetching the real
        // "settings" window by label.
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("MockRuntime can build a webview headlessly");

        let res = get_ipc_response(&webview, request("get_settings", serde_json::json!({})))
            .expect("get_settings should succeed under the real capabilities ACL");
        let value: serde_json::Value = res.deserialize().unwrap();

        assert_eq!(value["region_shortcut"], "CmdOrCtrl+Shift+R");
        assert_eq!(value["region_shortcut_pretty"], "⌘⇧R");
        assert_eq!(value["voice"], "F5");
        assert_eq!(value["speed"], 1.0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
