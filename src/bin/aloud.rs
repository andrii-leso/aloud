//! The Tauri menubar app: builds the TTS engine and `Player` once at
//! startup, registers the region hotkey and the macOS Service callback
//! (both funnel into `aloud::app::App`, which serializes them onto the
//! one shared `Player`), and exposes tray items for both actions plus
//! Stop.

// Without this, Windows gives a console-subsystem binary a console window, and
// Aloud — a tray app with no main window — launches with a stray black
// rectangle sitting on the desktop for its whole lifetime. `dumpbin /headers`
// reported `3 subsystem (Windows CUI)` before this line.
//
// Gated on `not(debug_assertions)` so a debug build keeps its console and
// `eprintln!` still reaches a terminal. In release the log file is the
// diagnostic channel (`src/log.rs`) — `eprintln!` from a GUI-subsystem process
// goes nowhere at all, which is exactly why that file exists.
//
// Crate-level, so it applies to this binary only: `src/bin/aloud_say.rs` is a
// CLI and must stay on the console subsystem.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use aloud::app::actions::Outcome;
use aloud::app::{App, SelectionOutcome};
use aloud::login_item::{LoginItemService, LoginItemStatus};
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

// The three platform seams, bound once here so nothing below this point
// mentions a platform. Same rule as `capture/mod.rs`: a `#[cfg]` past the
// seam means the seam is in the wrong place (Aloud hard constraint 8).
// The concrete types are interchangeable by construction — `Selector`
// and `Ocr` are the `RegionSelector`/`OcrEngine` impls with matching
// `new()` signatures, and `LoginItem` implements `LoginItemService`.
#[cfg(target_os = "macos")]
use aloud::capture::macos::ScreenCapture as Selector;
#[cfg(target_os = "windows")]
use aloud::capture::windows::ScreenCapture as Selector;

#[cfg(target_os = "macos")]
use aloud::ocr::macos::VisionOcr as Ocr;
#[cfg(target_os = "windows")]
use aloud::ocr::windows::WindowsOcr as Ocr;

#[cfg(target_os = "macos")]
use aloud::login_item::macos::AppServiceLoginItem as LoginItem;
#[cfg(target_os = "windows")]
use aloud::login_item::windows::UnsupportedLoginItem as LoginItem;

/// Opens the OS pane where the user manages Aloud's login item.
///
/// Split rather than called directly so `open_login_items_settings`
/// below stays platform-free. The Windows arm only logs — see
/// `src/login_item/windows.rs`.
#[cfg(target_os = "macos")]
use aloud::login_item::macos::open_system_settings_login_items;
#[cfg(target_os = "windows")]
use aloud::login_item::windows::open_system_settings_login_items;

/// The engine and `App` constructor arguments that a loaded `Settings`
/// implies.
///
/// This exists to be testable. The real wiring lives inside `setup()`,
/// which cannot be reached without a live `tauri::App`, and until M4's
/// final review nothing stood between "the saved voice and speed are
/// applied at launch" and "they are silently ignored" except reading the
/// code — which read fine while `setup()` in fact built the engine from a
/// hardcoded `VOICE` const and `App` from a hardcoded `SPEED`. Both
/// happened to equal the shipped defaults, so the app spoke F5 at 1.0x on
/// every launch while `get_settings` kept returning the saved M5 at 1.5x
/// and the settings window kept showing them. See `startup_config_tests`.
struct StartupConfig<'a> {
    voice: &'a str,
    speed: f32,
}

impl<'a> StartupConfig<'a> {
    fn from_settings(s: &'a Settings) -> Self {
        Self {
            voice: &s.voice,
            speed: s.speed,
        }
    }
}

/// Menubar tray icon, embedded rather than loaded from a path at runtime —
/// a path-based load can resolve differently once bundled inside the
/// `.app` (relative to whatever the process's cwd happens to be at
/// launch) and would fail silently in exactly the place this is hardest
/// to debug (no window, no attached terminal). 44x44px, drawn as a macOS
/// *template* image (pure black shapes, alpha-only anti-aliasing, no
/// colour) — paired with `.icon_as_template(true)` below so macOS
/// recolours it correctly for both light and dark menu bars. See
/// `icons/tray.png` and the Swift script that drew it.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../../icons/tray.png");

/// The Windows tray glyph, which must be a **different asset**, for two
/// independent reasons.
///
/// Size: Tauri's only public tray API is `Icon::from_rgba`, which calls Win32
/// `CreateIcon` at the source image's exact pixel dimensions. It never reads
/// `icons/icon.ico` and never calls `LoadIconMetric`, so the shell scales
/// whatever it is handed — and Windows' tray base size is 16 px, against
/// macOS's 44 px asset. Colour: `tray.png` is a macOS *template* image (pure
/// black on transparent) which only macOS recolours; on Windows it would be
/// black-on-black against the default dark taskbar.
///
/// 16 px is the 100%-scale rung. `icons/make_windows_icons.py` also emits the
/// 20/24/32 px rungs (125% / 150% / 200%) for a later runtime DPI swap; wiring
/// that swap is Windows-side work.
#[cfg(target_os = "windows")]
const TRAY_ICON: &[u8] = include_bytes!("../../icons/tray-windows-16.png");

/// Everything a hotkey handler or tray click needs, built once at startup
/// and shared for the life of the process.
struct Runtime {
    app: App,
    selector: Selector,
    ocr: Ocr,
    tray: TrayIcon<tauri::Wry>,
    status_item: MenuItem<tauri::Wry>,
    /// Handle to the tray's "Read Region" item, so `refresh_tray_labels`
    /// can update its label after a rebind without rebuilding the menu.
    read_region_item: MenuItem<tauri::Wry>,
    /// Handle to the tray's Pause/Resume item. Its label is rendered from
    /// `App::is_paused()` — see `refresh_pause_label`. This item is the
    /// only pause control that works with no selection: ⌃⌘S pauses too,
    /// but it arrives as a macOS Service, and macOS will not invoke a
    /// Service with nothing selected.
    pause_item: MenuItem<tauri::Wry>,
    /// Retained so a live voice swap (`spawn_voice_swap`) can build a new
    /// `Player` around the same audio output rather than opening a second
    /// device — only the engine changes.
    sink: Arc<dyn AudioSink>,
    /// The accelerator actually registered with the OS's global-shortcut
    /// plugin right now, or `None` if nothing is. This is ground truth for
    /// `apply_shortcut`'s rollback/no-op decisions, and it is deliberately
    /// NOT the same thing as `SettingsState.settings.region_shortcut` (the
    /// persisted, user-facing value) — those two can diverge (e.g. a
    /// persisted chord that failed to register at startup), and treating
    /// them as interchangeable was the Task 9 fix-round-1 bug: see
    /// `apply_shortcut`'s doc comment.
    registered_shortcut: Mutex<Option<String>>,
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
/// underway, the second because a cancel is not a failure. A read that
/// runs to the end resets the status to `Ready` (clearing any stale
/// error); one that was stopped before the end — displaced by a ⌃⌘S
/// takeover, or the tray's Stop — deliberately does not, since it did not
/// complete and whatever replaced it will report for itself. An
/// empty result (captured something, found no text) and any `Err` (most
/// importantly the missing Screen Recording permission from Task 3,
/// whose message already names System Settings and the required
/// restart — see `src/capture/macos.rs`) are surfaced, since both look
/// identical to "the hotkey did nothing" otherwise.
fn spawn_read_region(rt: Arc<Runtime>) {
    std::thread::spawn(move || {
        let outcome = rt.app.read_region(&rt.selector, &rt.ocr);
        // Whatever happened, the read is over, so nothing is paused any
        // more (an error unwind reaches `sink.stop()`, which clears it).
        // Re-render before reporting, so the tray can never be left
        // offering "Resume" for a read that has ended.
        refresh_pause_label(&rt);
        match outcome {
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
            Ok(Some(Outcome::Interrupted)) => {
                // Displaced by a ⌃⌘S takeover, or the tray's Stop.
                // Deliberately does NOT reset the status: this read did
                // not complete, and whatever replaced it is speaking now
                // and will report for itself.
                aloud::log_line!("read_region: stopped before the end (displaced or stopped)");
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
        }
    });
}

/// The two OS operations `apply_shortcut_with` needs, seamed out — same
/// pattern as `RegionSelector`/`OcrEngine`/`TtsEngine`/`AudioSink`
/// elsewhere in this crate — so the registered-shortcut bookkeeping that
/// closes the Task 9 fix-round-1 bug (see `apply_shortcut`'s doc comment)
/// has a unit test that needs no live `tauri::AppHandle`/global-shortcut
/// plugin to construct.
trait ShortcutRegistrar {
    fn register(&self, accel: &str) -> Result<(), String>;
    fn unregister(&self, accel: &str) -> Result<(), String>;
}

/// The real registrar: `tauri_plugin_global_shortcut`'s `AppHandle`
/// extension, wrapped so `apply_shortcut_with` never depends on the
/// concrete plugin type.
struct GlobalShortcutRegistrar<'a>(&'a tauri::AppHandle);

impl ShortcutRegistrar for GlobalShortcutRegistrar<'_> {
    fn register(&self, accel: &str) -> Result<(), String> {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        self.0
            .global_shortcut()
            .register(accel)
            .map_err(|e| e.to_string())
    }
    fn unregister(&self, accel: &str) -> Result<(), String> {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        self.0
            .global_shortcut()
            .unregister(accel)
            .map_err(|e| e.to_string())
    }
}

/// Moves the region hotkey to `new`, rolling back if it will not
/// register. Thin wrapper around `apply_shortcut_with` for the real OS —
/// see that function for the logic and for why `old` is no longer a
/// parameter here.
fn apply_shortcut(app: &tauri::AppHandle, rt: &Runtime, new: &str) -> Result<(), String> {
    apply_shortcut_with(&GlobalShortcutRegistrar(app), &rt.registered_shortcut, new)
}

/// Moves the region hotkey to `new`, rolling back to the previous one if
/// `new` will not register.
///
/// `old` — what `plan_apply` diffs against, and what a failed `new` rolls
/// back to — is read from `registered`, not supplied by the caller. It
/// used to be: the one call site that ever passed `Some` (`set_shortcut`)
/// sourced it from `SettingsState.region_shortcut`, the *persisted* value.
/// That is wrong whenever persisted and actually-registered diverge — for
/// example a chord that failed to register at startup (both it and the
/// default rejected): `SettingsState` still names the dead chord, so a
/// user "confirming" that exact chord in Settings made `plan_apply` see
/// `old == new` and return its empty no-op plan. `register()` was never
/// called at all, while `set_shortcut` still reported success and
/// persisted — the hotkey stayed permanently dead, and the later liveness
/// probe blamed a third-party app for the silence instead (Task 9 fix
/// round 1). `registered` cannot drift this way: every step below writes
/// it at the moment that step's real OS outcome is known, so it is never
/// inferred from what the user asked to save.
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
fn apply_shortcut_with(
    registrar: &impl ShortcutRegistrar,
    registered: &Mutex<Option<String>>,
    new: &str,
) -> Result<(), String> {
    use aloud::shortcut::Step;

    // The one screen every path to `register()` passes through. The
    // recorded-chord path is already screened by `Chord::to_accelerator`,
    // but the startup path takes `settings.region_shortcut` verbatim, so a
    // hand-edited settings.json could otherwise register a media key —
    // which routes into `start_watching_media_keys` and creates a
    // session-level `CGEventTapCreate` tap, the one thing that makes macOS
    // demand Accessibility / Input Monitoring. `Settings::normalize`
    // already strips such a value on load and save; this is the guard that
    // does not depend on the value having come from `Settings` at all.
    if aloud::shortcut::is_media_accelerator(new) {
        return Err(aloud::shortcut::ChordError::MediaKey.to_string());
    }

    let old = registered.lock().unwrap().clone();
    for step in aloud::shortcut::plan_apply(old.as_deref(), new) {
        match step {
            Step::Unregister(s) => {
                // Returns Ok for a chord that was never registered, so a
                // failure here is a real one.
                if let Err(e) = registrar.unregister(s.as_str()) {
                    return Err(format!("could not release {s}: {e}"));
                }
                *registered.lock().unwrap() = None;
            }
            Step::Register(s) => {
                if let Err(e) = registrar.register(s.as_str()) {
                    if let Some(o) = old.as_deref() {
                        if registrar.register(o).is_err() {
                            // Both the new chord and the rollback failed:
                            // the app now has NO region hotkey. Say so,
                            // rather than reporting the ordinary "new
                            // chord rejected" case.
                            *registered.lock().unwrap() = None;
                            return Err(format!(
                                "could not register {s}, and restoring {o} also failed - \
                                 there is currently no region shortcut. Open Settings and pick one."
                            ));
                        }
                        // Rollback succeeded: the OS (and the tracked
                        // state) is back on `old`.
                        *registered.lock().unwrap() = Some(o.to_string());
                    }
                    return Err(format!("could not register {s}: {e}"));
                }
                *registered.lock().unwrap() = Some(s);
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
#[cfg(not(target_os = "windows"))]
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

/// `"CmdOrCtrl+Shift+R"` → `"Ctrl+Shift+R"`. Display only.
///
/// Windows has no ⌘ key and does not use glyph accelerators — the platform
/// convention is spelled-out words joined by `+`. Rendering the macOS glyphs
/// here told the user to press a key their keyboard does not have.
///
/// `CmdOrCtrl` is the *cross-platform* token and resolves to Control here, so
/// it maps to `Ctrl`. A literal `Cmd`/`Super`/`Meta` is the Windows key and is
/// spelled `Win` — the two must not collapse, or a chord the user cannot press
/// would be displayed as one they can.
#[cfg(target_os = "windows")]
fn pretty_accelerator(accel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut key = "";
    for part in accel.split('+') {
        let name = match part.to_ascii_lowercase().as_str() {
            "cmdorctrl" | "control" | "ctrl" => "Ctrl",
            "cmd" | "command" | "super" | "meta" => "Win",
            "alt" | "option" => "Alt",
            "shift" => "Shift",
            _ => {
                key = part;
                continue;
            }
        };
        // `CmdOrCtrl+Control` is degenerate here — the glyph rendering keeps
        // ⌘ and ⌃ distinct, but on Windows both collapse to Ctrl and a naive
        // push emits "Ctrl+…+Ctrl".
        if !parts.contains(&name) {
            parts.push(name);
        }
    }
    // A missing or empty key renders as nothing, never a placeholder and never
    // a dangling separator — the same contract the macOS arm's tests pin down.
    if !key.is_empty() {
        parts.push(key);
    }
    parts.join("+")
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
    /// Exactly what is (to be) on disk. `Settings::save` serializes the
    /// WHOLE struct, so anything written here is persisted by the very
    /// next save from any command — see `active_shortcut`.
    settings: Mutex<Settings>,
    /// The region chord actually in force, which is what the settings
    /// window displays. Normally identical to `settings.region_shortcut`;
    /// it diverges when a persisted chord fails to register at launch and
    /// the app falls back to the default *deliberately without persisting
    /// it*, so a later launch can retry the saved one.
    ///
    /// That fallback used to be written into `settings` itself. Because
    /// `set_voice` and `set_speed` both go through `persist`, which saves
    /// the entire struct, nudging the speed slider once then wrote the
    /// fallback over the user's saved chord — permanently, with no retry
    /// ever. Keeping "what is persisted" and "what is active" in separate
    /// fields is what makes that unrepresentable rather than merely
    /// avoided.
    ///
    /// Not the same thing as `Runtime.registered_shortcut` either: that is
    /// OS ground truth (and `None` when nothing at all is registered),
    /// reachable only with an `AppHandle`, which the `get_settings`
    /// command deliberately does not take (see `command_tests`).
    active_shortcut: Mutex<String>,
    config_dir: std::path::PathBuf,
}

/// Mutates the persisted settings and writes them to disk. The single
/// write path for `set_shortcut`/`set_voice`/`set_speed`.
///
/// `Settings::save` serializes the whole struct, so every field in
/// `SettingsState.settings` is persisted by any one of these calls — which
/// is exactly why the startup shortcut fallback writes `active_shortcut`
/// instead of this (see `SettingsState`).
fn persist<T>(state: &SettingsState, f: impl FnOnce(&mut Settings) -> T) -> Result<T, String> {
    let mut s = state.settings.lock().unwrap();
    let out = f(&mut s);
    s.save(&state.config_dir).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Records the chord now in force *without* persisting it. See
/// `SettingsState::active_shortcut`.
fn set_active_shortcut(state: &SettingsState, accel: &str) {
    *state.active_shortcut.lock().unwrap() = accel.to_string();
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
    // The ACTIVE chord, not the persisted one: after a startup fallback
    // those differ, and the window must describe what the user's keyboard
    // will actually do. Taken (and released) before the `settings` lock so
    // the two are never held nested.
    let active = state.active_shortcut.lock().unwrap().clone();
    let s = state.settings.lock().unwrap();
    SettingsView {
        region_shortcut_pretty: pretty_accelerator(&active),
        region_shortcut: active,
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

    // `old` for apply_shortcut comes from `rt.registered_shortcut` (OS
    // ground truth), not from `state.settings.region_shortcut` (the
    // persisted value) — see `apply_shortcut`'s doc comment for the bug
    // that conflating them caused.
    let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
    if let Err(e) = apply_shortcut(&app, &rt, &accel) {
        // Whichever way it failed, `registered_shortcut` now holds OS
        // ground truth: the old chord if the rollback worked, `None` if it
        // did not. Re-sync the tray from that before returning — returning
        // early used to skip `refresh_tray_labels` entirely, so a failed
        // rollback left the menu advertising a chord that is no longer
        // registered. An empty accelerator renders as an empty pretty
        // string (see `pretty_accelerator`'s tests), which is honest here:
        // there is nothing to show.
        let active = rt
            .registered_shortcut
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
        refresh_tray_labels(&app, &active);
        return Err(e);
    }

    persist(&state, |s| s.region_shortcut = accel.clone())?;
    set_active_shortcut(&state, &accel);
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

/// The Pause/Resume item's label, for a given paused state. Pure, so the
/// one property that matters — the label names the action the click will
/// perform, never the state it is already in — is testable without a
/// tray. A "Pause" item on a paused read is the lying-control defect
/// class this app has spent the week removing.
///
/// No chord is shown. Pause has no global hotkey of its own: ⌃⌘S is the
/// keyboard route (see `intent::decide_selection`), and naming it here
/// would be a lie whenever nothing is selected, which is the one case
/// this item exists for.
fn pause_label(paused: bool) -> String {
    if paused { "Resume" } else { "Pause" }.to_string()
}

/// Re-renders the Pause/Resume label from ground truth.
///
/// Reads `App::is_paused()` (which reads the sink) rather than taking a
/// bool, so no call site can hand it a stale value. Must be called after
/// anything that can change the paused state: the tray's Pause/Resume
/// item, the ⌃⌘S toggle, Stop, and the end of a read.
fn refresh_pause_label(rt: &Runtime) {
    let _ = rt.pause_item.set_text(pause_label(rt.app.is_paused()));
}

/// Toggles pause and re-syncs the tray. The tray item's only entry
/// point, so it cannot update the state without also updating the label.
/// The ⌃⌘S route does not come through here — it toggles inside
/// `App::speak_selection` and its caller refreshes the label itself.
fn toggle_pause(rt: &Runtime) {
    let paused = rt.app.toggle_pause();
    aloud::log_line!("pause: now {}", if paused { "paused" } else { "playing" });
    refresh_pause_label(rt);
}

/// Validates and saves the requested voice, then starts the engine
/// rebuild on a background thread.
///
/// `Ok(())` means *saved and started*, nothing more — the ~1.4s rebuild
/// can still fail after this returns. Its real outcome arrives at the
/// settings page as an `aloud://voice-swapped` event; see
/// `spawn_voice_swap`.
#[tauri::command]
fn set_voice(app: AppHandle, state: State<'_, SettingsState>, voice: String) -> Result<(), String> {
    if !VOICES.contains(&voice.as_str()) {
        return Err(format!("unknown voice {voice}"));
    }
    persist(&state, |s| s.voice = voice.clone())?;
    spawn_voice_swap(&app, voice);
    Ok(())
}

/// What `spawn_voice_swap` reports back to the settings page once the
/// rebuild has actually finished. `error` is `None` on success.
#[derive(Clone, serde::Serialize)]
struct VoiceSwapResult {
    voice: String,
    error: Option<String>,
}

/// Delivers a `VoiceSwapResult` to the settings window if it is still
/// open. Same pattern as `aloud://probe-fired`: a closed window simply
/// misses it, and a failure is still carried by the tray status/tooltip.
fn emit_voice_swap_result(app: &tauri::AppHandle, result: VoiceSwapResult) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.emit("aloud://voice-swapped", result);
    }
}

/// Rebuilds the TTS engine on the requested voice and swaps it in.
///
/// Off the main thread: `SupertonicEngine::spawn` loads the voice style
/// and stands up a worker thread, which takes on the order of a second —
/// running it inline on the IPC call would freeze the settings page's
/// event loop for that long.
///
/// Because of that, `set_voice` returning `Ok` proves only that the choice
/// was saved. The page shows "Switching voice…" from the moment it invokes
/// `set_voice` until the `aloud://voice-swapped` event emitted below tells
/// it what actually happened — the page used to show "Ready." on
/// `set_voice`'s return instead, which was ~1.4s early on success and
/// simply false when the rebuild failed (a partial `~/.cache/supertonic3`
/// is enough), leaving green "Ready.", a saved voice, an unchanged engine,
/// and a tray tooltip nobody is looking at as the only signal.
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
                emit_voice_swap_result(&app, VoiceSwapResult { voice, error: None });
            }
            Err(e) => {
                aloud::log_line!("voice: could not switch to {voice}: {e:#}");
                let message = format!("Could not load the {voice} voice.");
                set_error_status(&rt, &message);
                emit_voice_swap_result(
                    &app,
                    VoiceSwapResult {
                        voice,
                        error: Some(message),
                    },
                );
            }
        }
    });
}

#[tauri::command]
fn set_speed(app: AppHandle, state: State<'_, SettingsState>, speed: f32) -> Result<f32, String> {
    let clamped = Settings::clamp_speed(speed);
    persist(&state, |s| s.speed = clamped)?;
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

/// Whether this build can read the current selection at all.
///
/// On macOS the OS hands Aloud the selected text through a system Service. On
/// Windows there is no equivalent channel — `src/selection/windows.rs` is a
/// deliberate stub — so the Read Selection section is inert, there is no
/// shortcut to change, and there is no pane to open either:
/// `open_system_shortcuts_pane` runs the macOS `open` binary with an
/// `x-apple.systempreferences:` URL, and its caller swallows the failure into a
/// log line. The button therefore rendered clickable, was clickable, and did
/// nothing anywhere the user could see.
///
/// The page asks rather than sniffing the platform itself, for the same reason
/// `get_login_item_status` exists: a control must render what is true, not what
/// the page assumes (hard constraint 12). `8a0732e` disabled the equivalent
/// tray item; this is the settings-window half of that same defect.
#[tauri::command]
fn selection_supported() -> bool {
    !cfg!(target_os = "windows")
}

/// The live `SMAppService` handle, in managed state so the two
/// launch-at-login commands can be driven by a fake in tests (they take
/// only `State`, never `AppHandle` — see `command_tests`).
struct LoginItemState(Box<dyn LoginItemService>);

/// What the settings page needs to render the launch-at-login toggle.
///
/// `on` is derived from the OS's live status, never from the persisted
/// `launch_at_login` bool — a user who switches Aloud off in System
/// Settings must see the toggle go off, and Aloud is not notified when
/// that happens. See `LoginItemStatus::is_on`.
#[derive(serde::Serialize)]
struct LoginItemView {
    on: bool,
    status: &'static str,
    note: Option<&'static str>,
}

impl From<LoginItemStatus> for LoginItemView {
    fn from(s: LoginItemStatus) -> Self {
        Self {
            on: s.is_on(),
            status: s.tag(),
            note: s.note(),
        }
    }
}

/// Reads the live login-item status. Called by the settings page every
/// time it loads, which is what keeps the toggle honest about a change
/// made outside Aloud.
#[tauri::command]
fn get_login_item_status(login_item: State<'_, LoginItemState>) -> LoginItemView {
    login_item.0.status().into()
}

/// Applies the requested launch-at-login state to the OS, then persists.
///
/// Same ordering as `set_shortcut`: nothing reaches disk until the OS has
/// accepted, so a rejected request cannot come back after a restart.
///
/// Returns the OS's own read-back status, not `enabled` — see
/// `login_item::apply`. `register()` returning `Ok` is not proof the app
/// will launch at login, and the toggle must show what is true rather
/// than what was asked for.
#[tauri::command]
fn set_launch_at_login(
    state: State<'_, SettingsState>,
    login_item: State<'_, LoginItemState>,
    enabled: bool,
) -> Result<LoginItemView, String> {
    let status = aloud::login_item::apply(login_item.0.as_ref(), enabled).map_err(|e| {
        aloud::log_line!("login item: could not set launch-at-login={enabled}: {e}");
        format!("Could not change launch at login: {e}")
    })?;

    persist(&state, |s| s.launch_at_login = enabled)?;
    aloud::log_line!("login item: launch-at-login={enabled}, status is now {status:?}");
    Ok(status.into())
}

/// Deep link to System Settings → General → Login Items — the only place
/// a `RequiresApproval` status can be resolved, since it means consent
/// was withheld or revoked and no amount of re-registering grants it.
///
/// Windows has no such pane to open (there is no login item), so the
/// settings page hides the button there and the platform arm only logs.
#[tauri::command]
fn open_login_items_settings() {
    open_system_settings_login_items();
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
    // Must match tauri.conf.json's `settings` window, which is what
    // every ordinary open actually uses (this branch only runs if the
    // config-created window was destroyed). Measured in a 480px
    // border-box (the window's inner width): the page is 629px tall in
    // its ordinary state and 682px with the launch-at-login failure note
    // and its Login Items button showing. The window is not resizable,
    // so anything shorter puts the last section — or a failure message —
    // below a fold nobody can scroll past comfortably. This is inner
    // size; the title bar adds ~28px on screen.
    .inner_size(480.0, 690.0)
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

/// Whether a `RunEvent::ExitRequested` should be prevented, i.e. the app
/// should keep running instead of quitting.
///
/// `code` is Tauri's own discriminator (see `RunEvent::ExitRequested`'s
/// doc comment): `None` means the request came from user interaction —
/// concretely, tao's `CloseRequested` -> `Destroyed` path firing because
/// the destroyed window was the last one Tauri was tracking. For a
/// menubar app with no Dock icon, the settings window IS that last
/// window, so closing it (the red X) used to quit the whole app instead
/// of just hiding its window — see
/// docs/2026-08-10-window-close-quits-app.md. `Some(_)` means a
/// deliberate `AppHandle::exit()` call: the tray's "Quit Aloud" item, or
/// `AppHandle::restart()`. Only the window-close case is prevented here;
/// a real quit must still exit cleanly, so this stays a plain predicate
/// rather than an unconditional `prevent_exit()`.
fn should_prevent_exit(code: Option<i32>) -> bool {
    code.is_none()
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
                    let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                    // The region chord is the only shortcut Aloud registers
                    // globally, so there is nothing to dispatch on here. If
                    // a second one is ever added, its dispatch must come
                    // BEFORE the probe check: the probe consumes the *next*
                    // press as proof the region chord is live and does not
                    // look at which shortcut arrived, so any other chord
                    // would falsely confirm a region chord that may in fact
                    // be shadowed by another app.
                    //
                    // A liveness probe swallows the press instead of reading
                    // a region.
                    if probe_consumed(app, shortcut) {
                        return;
                    }
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
            selection_supported,
            begin_probe,
            end_probe,
            get_login_item_status,
            set_launch_at_login,
            open_login_items_settings,
        ])
        .setup(|app| {
            // Menubar app: no Dock icon, no window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Loaded once, here, and wired into `SettingsState` below —
            // never loaded a second time.
            let config_dir = app.path().app_config_dir()?;
            let settings = Settings::load(&config_dir);

            // The saved voice and speed, not a hardcoded default — see
            // `StartupConfig` for the bug this closes.
            let startup = StartupConfig::from_settings(&settings);

            // Logged BEFORE the load, and with the reason it resolved that
            // way, because every way this goes wrong is silent. `model_dir()`
            // has three branches and the one that fires is invisible from the
            // outside: with `$ALOUD_MODEL_DIR` unset the app falls through to a
            // platform cache path that is usually EMPTY, and the only symptom
            // is that synthesis produces nothing. On Windows that is the
            // common case rather than the edge one, because a `$env:` variable
            // dies with the shell that set it and is not inherited by anything
            // launched from Explorer or the tray.
            //
            // The next line ("engine load complete") is the proof it worked, so
            // a log that has this line and not that one names the directory to
            // go and look at.
            match aloud::model_dir() {
                Ok(dir) => aloud::log_line!(
                    "model dir: {} (from {}, exists={})",
                    dir.display(),
                    if std::env::var_os("ALOUD_MODEL_DIR").is_some() {
                        "$ALOUD_MODEL_DIR"
                    } else {
                        "the platform default - $ALOUD_MODEL_DIR is not set"
                    },
                    dir.exists()
                ),
                Err(e) => aloud::log_line!("model dir: UNRESOLVED: {e}"),
            }

            // Paid once, here: the Supertonic model load is ~1.4s and must
            // happen at launch, not on the first hotkey press.
            let engine = Arc::new(SupertonicEngine::spawn(startup.voice)?);
            aloud::log_line!("engine load complete");
            // Typed as the trait object up front so the same handle can be
            // both handed to `Player::new` and retained on `Runtime` — a
            // live voice swap (`spawn_voice_swap`) reuses it rather than
            // opening a second audio device.
            let sink: Arc<dyn AudioSink> = Arc::new(RodioSink::new()?);
            let player = Player::new(engine, Arc::clone(&sink));
            let core = App::new(player, startup.speed);

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
            // than an instruction: ⌃⌘S already works, because Info.plist ships it as
            // the Service's NSKeyEquivalent. The previous label told the user to go
            // and assign it, which was untrue.
            //
            // "Read / Pause" rather than "Read": the same chord now pauses and
            // resumes the selection it started (see `App::speak_selection`), and
            // a label that still said only "Read" would understate what the one
            // control the user reaches for most actually does.
            #[cfg(not(target_os = "windows"))]
            let read_selection_item = MenuItem::with_id(
                app,
                "read_selection_info",
                "Read / Pause Selection  (⌃⌘S, or the Services menu)",
                false,
                None::<&str>,
            )?;

            // Windows has no Services menu and no system-mediated selection
            // channel at all, so there is no chord to name and nothing the user
            // could go and assign. Naming ⌃⌘S here told a Windows user to press
            // a key their keyboard does not have, for a feature that is not in
            // this build. Same rule as `UnsupportedLoginItem`: present and
            // honest beats absent, and beats a control that lies.
            #[cfg(target_os = "windows")]
            let read_selection_item = MenuItem::with_id(
                app,
                "read_selection_info",
                "Read Selection  (not available in this build)",
                false,
                None::<&str>,
            )?;

            // The selection shortcut is a macOS Service, so it can only be changed
            // in System Settings. This opens the exact pane instead of describing
            // where it is.
            #[cfg(not(target_os = "windows"))]
            let services_settings_item = MenuItem::with_id(
                app,
                "services_settings",
                "Change Selection Shortcut…",
                true,
                None::<&str>,
            )?;

            // Disabled on Windows. `open_system_shortcuts_pane` shells out to the
            // macOS `open` binary with an `x-apple.systempreferences:` URL, which
            // does not exist here — and the caller swallows the error into a log
            // line, so the item rendered ENABLED, was clickable, and silently did
            // nothing. There is also no pane to open: the selection mechanism is
            // designed (UI Automation) but not built. Hard constraint 12 — a
            // control shows what is true, never what was requested.
            #[cfg(target_os = "windows")]
            let services_settings_item = MenuItem::with_id(
                app,
                "services_settings",
                "Change Selection Shortcut…",
                false,
                None::<&str>,
            )?;
            let separator2 = PredefinedMenuItem::separator(app)?;

            let settings_item =
                MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            // Built as "Pause" because nothing is speaking at startup, so
            // nothing is paused. Every later change goes through
            // `refresh_pause_label`, which reads the sink.
            let pause_item =
                MenuItem::with_id(app, "pause", pause_label(false), true, None::<&str>)?;
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
                    &pause_item,
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
                        // ⌃⌘S already works; this is for users who want to change it, or
                        // whose own apps happen to claim that chord. A Service key
                        // equivalent LOSES to an app's own menu item and competes with
                        // other Services, so no default is collision-proof — which is
                        // why this deep link exists rather than a promise. See the
                        // Info.plist comment for why ⌃⌘S replaced ⌘⇧A.
                        if let Err(e) = open_system_shortcuts_pane() {
                            aloud::log_line!(
                                "services_settings: could not open System Settings: {e}"
                            );
                            let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                            set_error_status(&rt, &format!("Could not open System Settings: {e}"));
                        }
                    } else if event.id() == "settings" {
                        open_settings_window(app);
                    } else if event.id() == "pause" {
                        let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                        toggle_pause(&rt);
                    } else if event.id() == "stop" {
                        let rt = Arc::clone(app.state::<Arc<Runtime>>().inner());
                        rt.app.stop();
                        // Stop clears the paused state (see the
                        // `AudioSink::stop` contract), so a Stop pressed
                        // while paused would otherwise leave the item
                        // reading "Resume" with nothing to resume.
                        refresh_pause_label(&rt);
                    } else if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            let runtime = Arc::new(Runtime {
                app: core,
                selector: Selector::new(),
                ocr: Ocr::new()?,
                tray,
                status_item,
                read_region_item,
                pause_item,
                sink,
                // Nothing is registered with the OS yet — the block below
                // is what first does that.
                registered_shortcut: Mutex::new(None),
            });
            app.manage(runtime);
            app.manage(SettingsState {
                active_shortcut: Mutex::new(settings.region_shortcut.clone()),
                settings: Mutex::new(settings.clone()),
                config_dir,
            });

            // Launch at login. The OS is the source of truth and it is
            // read, never written, here: the state lives in macOS's
            // Background Task Management store, and the commonest reason
            // for it to disagree with what Aloud saved is the user
            // deliberately switching Aloud off in System Settings →
            // General → Login Items. Re-registering to "correct" that
            // would override an opt-out Aloud is never notified of, so
            // this only says what it found — the settings window's
            // toggle then renders the live status, and only an explicit
            // toggle ever registers. See src/login_item/mod.rs. On
            // Windows there is no mechanism yet, so `LoginItem` reports
            // `Unsupported` and this block just records that.
            let login_item = LoginItem;
            let live = login_item.status();
            aloud::log_line!(
                "login item: status={live:?}, settings say launch_at_login={}",
                settings.launch_at_login
            );
            if live.is_on() != settings.launch_at_login {
                aloud::log_line!(
                    "login item: settings and the OS disagree - the OS wins; \
                     nothing re-registered"
                );
            }
            app.manage(LoginItemState(Box::new(login_item)));

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
                match apply_shortcut(&handle, &rt, &wanted) {
                    Ok(()) => {
                        aloud::log_line!("hotkey: registered {wanted}");
                    }
                    Err(e) => {
                        aloud::log_line!("hotkey: failed to register {wanted}: {e}");
                        if wanted != aloud::settings::DEFAULT_SHORTCUT {
                            match apply_shortcut(&handle, &rt, aloud::settings::DEFAULT_SHORTCUT) {
                                Ok(()) => {
                                    aloud::log_line!(
                                        "hotkey: fell back to {}",
                                        aloud::settings::DEFAULT_SHORTCUT
                                    );
                                    // The OS now has the default
                                    // registered, not `wanted`, so the
                                    // tray label (built above from the
                                    // pre-fallback `settings`) and the
                                    // settings window both need to agree
                                    // with reality. But this failure is
                                    // presumed transient (e.g. another app
                                    // briefly holding the same chord)
                                    // rather than a permanent rejection —
                                    // `set_shortcut` already screens out
                                    // permanent rejections before anything
                                    // is ever saved — so `wanted` must
                                    // survive ON DISK for a later launch
                                    // to retry it.
                                    //
                                    // Hence `active_shortcut`, never
                                    // `settings`: writing the fallback
                                    // into the persisted struct (as this
                                    // did) meant the next `set_voice` or
                                    // `set_speed` — both of which save the
                                    // whole struct — silently overwrote
                                    // the user's saved chord with the
                                    // default, killing the retry forever.
                                    set_active_shortcut(
                                        &app.state::<SettingsState>(),
                                        aloud::settings::DEFAULT_SHORTCUT,
                                    );
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

            // Pause/resume has no global hotkey of its own, deliberately.
            // ⌃⌘S already pauses and resumes the read it started (see
            // `intent::decide_selection`), so a second chord would be
            // redundant surface — and the one it had, ⌘⇧P, is VS Code's
            // Command Palette, which a non-exclusive global hotkey wins
            // while Aloud runs. The tray's Pause/Resume item stays as the
            // fallback for the case ⌃⌘S cannot cover: macOS will not
            // invoke a Service with nothing selected.

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
                        std::thread::spawn(move || {
                            let outcome = rt.app.speak_selection(&text);
                            // Re-rendered from the sink on every path, not
                            // only when a read ends. A `Toggled` outcome
                            // returns while the read is still in flight and
                            // is the ONLY thing that changed the paused
                            // state, so the tray would otherwise keep
                            // offering "Pause" for an already-paused read.
                            refresh_pause_label(&rt);
                            match outcome {
                            Ok(SelectionOutcome::Spoke { replaced }) => {
                                aloud::log_line!(
                                    "speak_selection: completed, spoke{}",
                                    if replaced { " (replaced the read in flight)" } else { "" }
                                );
                                reset_status(&rt);
                            }
                            Ok(SelectionOutcome::Cut { replaced }) => {
                                // This read was itself displaced. No
                                // `reset_status`: it did not complete, and
                                // its replacement is already speaking.
                                aloud::log_line!(
                                    "speak_selection: stopped before the end{}",
                                    if replaced { " (had replaced the read in flight)" } else { "" }
                                );
                            }
                            Ok(SelectionOutcome::Toggled { paused }) => {
                                aloud::log_line!(
                                    "speak_selection: same selection delivered again, now {}",
                                    if paused { "paused" } else { "playing" }
                                );
                            }
                            Ok(SelectionOutcome::Empty) => {
                                aloud::log_line!(
                                    "speak_selection: nothing speakable in the delivered \
                                     selection; anything already playing was left alone"
                                );
                            }
                            Ok(SelectionOutcome::Skipped) => {
                                // Two different causes, and `App` has
                                // already logged which one — a takeover
                                // already under way (harmless), or a
                                // takeover that waited out TAKEOVER_WAIT
                                // having already silenced the audio (not
                                // harmless at all). Naming one of them
                                // here would make the log carry a true
                                // line and a false one about the same
                                // event.
                                aloud::log_line!("speak_selection: skipped, see the reason above");
                            }
                            Err(e) => {
                                eprintln!("[aloud] speak_selection failed: {e:#}");
                                aloud::log_line!("speak_selection: error: {e:#}");
                                set_error_status(&rt, &e.to_string());
                            }
                            }
                        });
                    },
                ))?;
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running Aloud")
        .run(|_app_handle, event| {
            // Tauri's default `RunEvent` handling (what `Builder::run`
            // provided when nobody supplied a callback, which is what this
            // app used to do implicitly) exits the whole process the
            // instant the last open window is destroyed — see
            // `should_prevent_exit` for why that is wrong here.
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                if should_prevent_exit(code) {
                    api.prevent_exit();
                }
            }
        });
}

/// Covers `should_prevent_exit` in isolation (the code/no-code
/// discrimination) and, via `MockRuntime`, the real `Builder` -> `build`
/// -> `run` wiring for the window-close half of the bug: closing the
/// settings window must not end the run loop.
///
/// What this file CANNOT cover: `MockRuntime::request_exit` — what
/// `AppHandle::exit()` calls, i.e. exactly what the tray's "Quit Aloud"
/// item triggers — is `unimplemented!()` in tauri 2.11.5's test runtime
/// (panics if called). So the "a deliberate quit must still exit
/// cleanly" half of the fix has no automated regression test at this
/// layer; it was verified by hand against the real release binary
/// (`app_handle.exit(0)` called programmatically, not clicked) — see
/// docs/2026-08-10-window-close-quits-app.md for the transcript, and
/// re-verify by hand if this run-loop code changes again.
#[cfg(test)]
mod run_event_tests {
    use super::should_prevent_exit;

    #[test]
    fn window_close_is_prevented_but_a_coded_exit_is_not() {
        assert!(
            should_prevent_exit(None),
            "a window-close exit request (code: None) must be prevented, \
             or closing the settings window quits the whole app"
        );
        assert!(
            !should_prevent_exit(Some(0)),
            "a deliberate AppHandle::exit() (code: Some(_)) must NOT be \
             prevented, or the tray's Quit Aloud item stops working"
        );
    }

    #[test]
    fn closing_the_only_window_does_not_end_the_run_loop() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let w = tauri::WebviewWindowBuilder::new(&app, "settings", Default::default())
            .build()
            .unwrap();

        // `app.run()` blocks until the loop actually exits, so it has to
        // run on its own thread; `done_rx` is how the test observes
        // whether (and when) that happened.
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            app.run(|_app_handle, event| {
                if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                    if should_prevent_exit(code) {
                        api.prevent_exit();
                    }
                }
            });
            let _ = done_tx.send(());
        });

        w.close().unwrap();

        // MockRuntime's loop polls its message queue once per second
        // (see tauri's mock_runtime.rs), so give it several iterations to
        // actually process the close before concluding it survived.
        assert!(
            done_rx.recv_timeout(Duration::from_secs(3)).is_err(),
            "run() returned after the settings window closed — the real \
             app would have quit here"
        );
        // The spawned `app.run()` is now blocked forever (by design: this
        // test proves it never receives another exit request), so there
        // is nothing to join — the thread is reaped when the test binary
        // exits.
    }
}

/// Characterisation tests for `pretty_accelerator`. Its inverse,
/// `Chord::to_accelerator` (`src/shortcut.rs`), already has a full suite in
/// `tests/shortcut.rs`; this covers the same class of case for the display
/// side, since Task 6 extends `pretty_accelerator` to render arbitrary
/// user-entered chords and a careless edit here would otherwise go
/// uncaught until someone noticed a wrong glyph on screen.
/// The glyph rendering is macOS-only. Windows spells modifiers out, so these
/// assertions are gated rather than changed — the macOS behaviour they pin
/// down is unaltered, and `windows_accelerator_tests` below mirrors every case
/// for the other arm so neither platform loses coverage.
#[cfg(all(test, not(target_os = "windows")))]
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

/// The Windows arm of `pretty_accelerator`, case for case against `tests`
/// above. Windows has no ⌘ key and does not use glyph accelerators — the
/// platform convention is spelled-out words joined by `+` — so the *rendering*
/// differs while the *contract* (order preserved, key casing untouched,
/// malformed input never panics and never grows a placeholder) is identical.
#[cfg(all(test, target_os = "windows"))]
mod windows_accelerator_tests {
    use super::pretty_accelerator;

    #[test]
    fn default_region_shortcut_renders_as_the_shipped_menu_string() {
        // The string a Windows user actually sees in the tray. `CmdOrCtrl`
        // resolves to Control on this platform, so it must render Ctrl — the
        // glyph form told them to press a key their keyboard does not have.
        assert_eq!(pretty_accelerator("CmdOrCtrl+Shift+R"), "Ctrl+Shift+R");
    }

    #[test]
    fn all_four_modifiers_render_in_the_order_to_accelerator_emits_them() {
        // Same fixed order as `Chord::to_accelerator`. Note `CmdOrCtrl` and
        // `Control` BOTH mean Ctrl here, where macOS renders them as distinct
        // glyphs (⌘ and ⌃) — so the duplicate is collapsed rather than emitted
        // twice. This exact input produced "Ctrl+Alt+Ctrl+Shift+R" before the
        // dedup landed.
        assert_eq!(
            pretty_accelerator("CmdOrCtrl+Alt+Control+Shift+R"),
            "Ctrl+Alt+Shift+R"
        );
    }

    /// A literal Cmd/Super/Meta is the WINDOWS key, not Control. Collapsing it
    /// into Ctrl would display a chord the user can press in place of one they
    /// cannot — the opposite of the bug this whole split fixes.
    #[test]
    fn the_meta_key_is_win_not_ctrl() {
        assert_eq!(pretty_accelerator("Super+Shift+R"), "Win+Shift+R");
        assert_eq!(pretty_accelerator("Meta+R"), "Win+R");
    }

    #[test]
    fn multi_character_key_tokens_keep_their_original_casing() {
        assert_eq!(
            pretty_accelerator("CmdOrCtrl+Shift+ArrowUp"),
            "Ctrl+Shift+ArrowUp"
        );
        assert_eq!(pretty_accelerator("Alt+F7"), "Alt+F7");
    }

    #[test]
    fn modifier_matching_is_case_insensitive_but_the_key_is_passed_through_as_is() {
        assert_eq!(pretty_accelerator("cmdorctrl+shift+r"), "Ctrl+Shift+r");
    }

    // Same malformed-input contract as the macOS arm: modifiers found are
    // still rendered, and a missing or empty key renders as nothing — not a
    // placeholder, not an error, and (the Windows-specific trap) not a
    // dangling "+". The naive `parts.push(key); parts.join("+")` produced
    // "Ctrl+" and "Shift+" for the last two.
    #[test]
    fn empty_input_yields_an_empty_string() {
        assert_eq!(pretty_accelerator(""), "");
    }

    #[test]
    fn trailing_separator_with_no_key_drops_the_key_silently() {
        assert_eq!(pretty_accelerator("CmdOrCtrl+"), "Ctrl");
    }

    #[test]
    fn modifiers_only_with_no_key_renders_just_the_modifiers() {
        assert_eq!(pretty_accelerator("Shift"), "Shift");
    }
}

/// `pause_label` decides what the tray's Pause/Resume item says, and the
/// one thing it must never do is describe the state instead of the
/// action — a "Pause" item on an already-paused read is exactly the
/// lying-control defect this project has spent the week removing. The
/// label is only ever produced here, and only ever from
/// `App::is_paused()` (see `refresh_pause_label`), so this is the whole
/// truthfulness surface.
/// Split by platform rather than written as
/// `assert_eq!(selection_supported(), !cfg!(target_os = "windows"))`, which
/// would only be testing the function against its own implementation. Each
/// arm pins the concrete expected answer, the same way the accelerator tests
/// do.
#[cfg(test)]
mod selection_support_tests {
    use super::selection_supported;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_has_no_selection_channel() {
        assert!(
            !selection_supported(),
            "the settings page renders Read Selection as live off this"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn the_macos_service_is_a_selection_channel() {
        assert!(selection_supported());
    }
}

#[cfg(test)]
mod pause_label_tests {
    use super::pause_label;

    #[test]
    fn names_the_action_the_click_performs_not_the_current_state() {
        assert_eq!(
            pause_label(false),
            "Pause",
            "while playing, the item must offer Pause"
        );
        assert_eq!(
            pause_label(true),
            "Resume",
            "while paused, the item must offer Resume - never 'Pause'"
        );
    }

    #[test]
    fn advertises_no_chord() {
        // Pause has no global hotkey. Naming ⌃⌘S here would be a lie in
        // exactly the case this item exists for — nothing selected, so
        // macOS will not invoke the Service and the chord cannot fire.
        for label in [pause_label(false), pause_label(true)] {
            assert!(
                !label.contains('('),
                "the pause item must not advertise a chord, got {label:?}"
            );
        }
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

    /// A throwaway config dir under the OS temp dir — never the real
    /// `~/Library/Application Support/com.andriileso.aloud/`, so these
    /// tests can never touch the owner's actual `settings.json`.
    fn throwaway_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aloud-command-tests-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Builds the mock app around `state` and invokes `get_settings`
    /// through the real IPC pipeline, returning the deserialized view.
    fn invoke_get_settings(state: SettingsState) -> serde_json::Value {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![get_settings])
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(state);
        // The noop context declares no windows (see the doc comment
        // above for why it's noop rather than this crate's real
        // context), so build one ad hoc rather than fetching the real
        // "settings" window by label.
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("MockRuntime can build a webview headlessly");

        let res = get_ipc_response(&webview, request("get_settings", serde_json::json!({})))
            .expect("get_settings should succeed under the real capabilities ACL");
        res.deserialize().unwrap()
    }

    /// `selection_supported` through the real IPC pipeline, not just as a
    /// plain call: the settings page reaches it by name, so what matters is
    /// that it is registered in `generate_handler!` and serializes to a bare
    /// bool. A command that exists but was never wired up fails exactly here
    /// and nowhere else — and on the page it would surface as the section
    /// silently keeping its macOS text.
    #[test]
    fn selection_supported_is_reachable_over_ipc() {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![selection_supported])
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("MockRuntime can build a webview headlessly");

        let res = get_ipc_response(
            &webview,
            request("selection_supported", serde_json::json!({})),
        )
        .expect("selection_supported should succeed under the real capabilities ACL");
        let value: bool = res.deserialize().unwrap();
        assert_eq!(value, !cfg!(target_os = "windows"));
    }

    /// `DEFAULT_SHORTCUT` as the settings window renders it. Split for the
    /// same reason `pretty_accelerator` is: macOS uses glyphs, Windows spells
    /// the modifiers out. Kept as a literal rather than a call to
    /// `pretty_accelerator` so the assertion still pins the expected output
    /// instead of testing the function against itself.
    #[cfg(not(target_os = "windows"))]
    const DEFAULT_SHORTCUT_PRETTY: &str = "⌘⇧R";
    #[cfg(target_os = "windows")]
    const DEFAULT_SHORTCUT_PRETTY: &str = "Ctrl+Shift+R";

    #[test]
    fn get_settings_returns_the_expected_shape() {
        let dir = throwaway_dir("get");
        let value = invoke_get_settings(SettingsState {
            settings: Mutex::new(Settings::default()),
            active_shortcut: Mutex::new(Settings::default().region_shortcut),
            config_dir: dir.clone(),
        });

        assert_eq!(value["region_shortcut"], "CmdOrCtrl+Shift+R");
        assert_eq!(value["region_shortcut_pretty"], DEFAULT_SHORTCUT_PRETTY);
        assert_eq!(value["voice"], "F5");
        assert_eq!(value["speed"], 1.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_settings_reports_the_active_chord_not_the_persisted_one() {
        // The post-startup-fallback state: "Alt+Shift+E" is what is saved
        // and what a later launch will retry, but the default is what the
        // OS actually has right now. The window must show what pressing a
        // key will really do — showing the saved-but-dead chord is how a
        // settings page becomes a lie about its own app.
        let dir = throwaway_dir("active");
        let value = invoke_get_settings(SettingsState {
            settings: Mutex::new(Settings {
                region_shortcut: "Alt+Shift+E".into(),
                ..Settings::default()
            }),
            active_shortcut: Mutex::new(aloud::settings::DEFAULT_SHORTCUT.to_string()),
            config_dir: dir.clone(),
        });

        assert_eq!(value["region_shortcut"], "CmdOrCtrl+Shift+R");
        assert_eq!(value["region_shortcut_pretty"], DEFAULT_SHORTCUT_PRETTY);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `LoginItemService` with a scripted register outcome and a
    /// scripted status. The two are independent on purpose: "the OS said
    /// yes" and "the OS will actually do it" are different facts, and
    /// conflating them is the defect these commands exist to avoid.
    struct FakeLoginItem {
        register_result: Result<(), aloud::login_item::LoginItemError>,
        status: LoginItemStatus,
    }

    impl LoginItemService for FakeLoginItem {
        fn status(&self) -> LoginItemStatus {
            self.status
        }
        fn register(&self) -> Result<(), aloud::login_item::LoginItemError> {
            self.register_result.clone()
        }
        fn unregister(&self) -> Result<(), aloud::login_item::LoginItemError> {
            self.register_result.clone()
        }
    }

    /// Builds the mock app around both states and invokes one of the
    /// launch-at-login commands through the real IPC pipeline.
    fn invoke_login_item(
        state: SettingsState,
        fake: FakeLoginItem,
        cmd: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, serde_json::Value> {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![
                get_login_item_status,
                set_launch_at_login
            ])
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(state);
        app.manage(LoginItemState(Box::new(fake)));
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("MockRuntime can build a webview headlessly");

        get_ipc_response(&webview, request(cmd, body)).map(|r| r.deserialize().unwrap())
    }

    #[test]
    fn the_toggle_reads_the_live_os_status_not_the_saved_bool() {
        // The state after a user switches Aloud off in System Settings →
        // General → Login Items: Aloud is never notified, so its saved
        // bool still says true while the OS says otherwise. Rendering the
        // saved bool here is exactly how a settings window comes to lie
        // about the system it is describing.
        let dir = throwaway_dir("login-live");
        let value = invoke_login_item(
            SettingsState {
                settings: Mutex::new(Settings {
                    launch_at_login: true,
                    ..Settings::default()
                }),
                active_shortcut: Mutex::new(Settings::default().region_shortcut),
                config_dir: dir.clone(),
            },
            FakeLoginItem {
                register_result: Ok(()),
                status: LoginItemStatus::RequiresApproval,
            },
            "get_login_item_status",
            serde_json::json!({}),
        )
        .expect("get_login_item_status should succeed");

        assert_eq!(value["on"], false);
        assert_eq!(value["status"], "requires_approval");
        assert!(
            value["note"]
                .as_str()
                .is_some_and(|n| n.contains("System Settings")),
            "the one state the user has to fix themselves must say where"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rejected_request_is_not_written_to_disk() {
        // Same ordering rule as `set_shortcut`: the OS decides first, and
        // only an accepted change is persisted. Persisting first would
        // leave settings.json claiming a login item that does not exist,
        // for every future launch to report as a disagreement.
        let dir = throwaway_dir("login-rejected");
        Settings::default().save(&dir).unwrap();

        let result = invoke_login_item(
            SettingsState {
                settings: Mutex::new(Settings::default()),
                active_shortcut: Mutex::new(Settings::default().region_shortcut),
                config_dir: dir.clone(),
            },
            FakeLoginItem {
                register_result: Err(aloud::login_item::LoginItemError {
                    domain: "SMAppServiceErrorDomain".into(),
                    code: 1,
                    message: "Operation not permitted".into(),
                }),
                status: LoginItemStatus::Enabled,
            },
            "set_launch_at_login",
            serde_json::json!({ "enabled": true }),
        );

        assert!(
            result.is_err(),
            "a refused registration must surface as an error"
        );
        assert!(
            !Settings::load(&dir).launch_at_login,
            "nothing may reach disk until the OS has accepted"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_accepted_request_persists_and_reports_the_read_back_status() {
        let dir = throwaway_dir("login-accepted");
        Settings::default().save(&dir).unwrap();

        let value = invoke_login_item(
            SettingsState {
                settings: Mutex::new(Settings::default()),
                active_shortcut: Mutex::new(Settings::default().region_shortcut),
                config_dir: dir.clone(),
            },
            FakeLoginItem {
                register_result: Ok(()),
                status: LoginItemStatus::Enabled,
            },
            "set_launch_at_login",
            serde_json::json!({ "enabled": true }),
        )
        .expect("set_launch_at_login should succeed");

        assert_eq!(value["on"], true);
        assert_eq!(value["status"], "enabled");
        assert!(value["note"].is_null());
        assert!(Settings::load(&dir).launch_at_login);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Regression coverage for the M4 whole-branch bug: `Settings::load` ran
/// at startup, but the engine was built from a hardcoded `VOICE` const and
/// `App` from a hardcoded `SPEED`, so a saved voice and speed were applied
/// live, persisted, reported back by `get_settings` — and then silently
/// dropped on the next launch. Both consts happened to equal the shipped
/// defaults, which is the only reason nothing looked wrong.
///
/// `setup()` itself needs a live `tauri::App`, so the mapping it now uses
/// is what is asserted here; `setup()` reads `startup.voice`/`startup.speed`
/// and nothing else when constructing the engine and `App`.
#[cfg(test)]
mod startup_config_tests {
    use super::StartupConfig;
    use aloud::settings::{Settings, DEFAULT_SPEED, DEFAULT_VOICE};

    #[test]
    fn a_saved_non_default_voice_and_speed_reach_construction() {
        let settings = Settings {
            region_shortcut: aloud::settings::DEFAULT_SHORTCUT.to_string(),
            voice: "M5".into(),
            speed: 1.5,
            ..Settings::default()
        };
        // Both deliberately differ from the defaults: an implementation
        // that ignored `settings` and returned the hardcoded defaults —
        // exactly the bug — would still satisfy an assertion written
        // against a default-valued `Settings`.
        assert_ne!(settings.voice, DEFAULT_VOICE);
        assert_ne!(settings.speed, DEFAULT_SPEED);

        let c = StartupConfig::from_settings(&settings);
        assert_eq!(
            c.voice, "M5",
            "the engine must be built on the saved voice, not the default"
        );
        assert_eq!(
            c.speed, 1.5,
            "App must be constructed with the saved speed, not the default"
        );
    }

    #[test]
    fn a_default_settings_still_produces_the_shipped_defaults() {
        let settings = Settings::default();
        let c = StartupConfig::from_settings(&settings);
        assert_eq!(c.voice, DEFAULT_VOICE);
        assert_eq!(c.speed, DEFAULT_SPEED);
    }
}

/// Regression coverage for the "a later voice/speed change persists the
/// startup fallback over the user's saved chord" bug.
///
/// `set_voice`/`set_speed` cannot be invoked here (they take `AppHandle`;
/// see `command_tests`), but their disk-write path is `persist`, and the
/// startup fallback's state update is `set_active_shortcut` — both are
/// driven directly below, in the order the real app runs them.
#[cfg(test)]
mod settings_state_tests {
    use super::*;

    #[test]
    fn a_startup_fallback_is_not_persisted_over_the_saved_chord_by_a_later_speed_change() {
        let dir = std::env::temp_dir().join(format!(
            "aloud-settings-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let saved = Settings {
            region_shortcut: "Alt+Shift+E".into(),
            voice: "F5".into(),
            speed: 1.0,
            ..Settings::default()
        };
        saved.save(&dir).unwrap();

        let state = SettingsState {
            active_shortcut: Mutex::new(saved.region_shortcut.clone()),
            settings: Mutex::new(saved.clone()),
            config_dir: dir.clone(),
        };

        // Launch: "Alt+Shift+E" would not register, so the default is in
        // force instead — in memory only, never persisted.
        set_active_shortcut(&state, aloud::settings::DEFAULT_SHORTCUT);

        // The user then nudges the speed slider. This is `set_speed`'s
        // entire disk-write path, and `Settings::save` writes the WHOLE
        // struct — which is how the fallback used to escape to disk.
        persist(&state, |s| s.speed = 1.5).unwrap();

        let on_disk = Settings::load(&dir);
        assert_eq!(
            on_disk.region_shortcut, "Alt+Shift+E",
            "the saved chord must survive an unrelated settings change, or \
             the next launch can never retry it"
        );
        assert_eq!(on_disk.speed, 1.5, "the speed change itself must persist");
        assert_eq!(
            state.active_shortcut.lock().unwrap().as_str(),
            aloud::settings::DEFAULT_SHORTCUT,
            "the fallback stays in force for this session"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Regression coverage for the Task 9 fix-round-1 bug: `apply_shortcut`
/// used to source `old` from `SettingsState.region_shortcut` (persisted)
/// instead of what was actually registered with the OS, which let a
/// stale persisted chord make `plan_apply` see a false no-op and silently
/// skip `register()` while still reporting success. `apply_shortcut_with`
/// — the core logic, seamed behind `ShortcutRegistrar` — is exercised
/// directly here with a `FakeRegistrar`, needing no live
/// `tauri::AppHandle` or global-shortcut plugin, unlike `apply_shortcut`
/// itself (concrete `&tauri::AppHandle`, same `MockRuntime` wall
/// documented on `command_tests` above).
#[cfg(test)]
mod shortcut_registration_tests {
    use super::*;

    /// A `ShortcutRegistrar` whose `register` outcome is scripted per
    /// accelerator (`unregister` always succeeds — nothing in this file's
    /// logic branches on it failing except by propagating the error
    /// as-is), and which records every call so a test can assert not just
    /// the final tracked state but which OS calls actually happened.
    struct FakeRegistrar {
        fails_to_register: Vec<String>,
        register_calls: Mutex<Vec<String>>,
        unregister_calls: Mutex<Vec<String>>,
    }

    impl FakeRegistrar {
        fn new() -> Self {
            Self::failing(&[])
        }

        fn failing(accels: &[&str]) -> Self {
            Self {
                fails_to_register: accels.iter().map(|s| s.to_string()).collect(),
                register_calls: Mutex::new(Vec::new()),
                unregister_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl ShortcutRegistrar for FakeRegistrar {
        fn register(&self, accel: &str) -> Result<(), String> {
            self.register_calls.lock().unwrap().push(accel.to_string());
            if self.fails_to_register.iter().any(|a| a == accel) {
                Err(format!("fake: {accel} rejected"))
            } else {
                Ok(())
            }
        }
        fn unregister(&self, accel: &str) -> Result<(), String> {
            self.unregister_calls
                .lock()
                .unwrap()
                .push(accel.to_string());
            Ok(())
        }
    }

    #[test]
    fn confirming_a_chord_still_registers_it_when_nothing_is_actually_registered() {
        // The bug, reproduced directly: `registered` tracks OS ground
        // truth (None — nothing is live), entirely independent of
        // whatever a persisted `Settings` value might say. This is
        // exactly the state after both a persisted chord and the default
        // fail to register at startup — `SettingsState.region_shortcut`
        // still names the dead chord, and the settings page pre-fills
        // the record button with it (dist/settings.js). Before the fix,
        // `set_shortcut` sourced `old` from that persisted value, so
        // confirming the very chord already on screen made `plan_apply`
        // see `old == new` and return its no-op empty plan — `register`
        // was never called, while `set_shortcut` still reported success.
        let registrar = FakeRegistrar::new();
        let registered: Mutex<Option<String>> = Mutex::new(None);

        let result = apply_shortcut_with(&registrar, &registered, "CmdOrCtrl+Shift+R");

        assert!(result.is_ok());
        assert_eq!(
            registrar.register_calls.lock().unwrap().as_slice(),
            ["CmdOrCtrl+Shift+R"],
            "register() must actually have been called — this is exactly \
             the call the old (broken) settings-sourced `old` would skip"
        );
        assert_eq!(
            registered.lock().unwrap().as_deref(),
            Some("CmdOrCtrl+Shift+R"),
            "the tracked state must reflect the real, now-successful registration"
        );
    }

    #[test]
    fn rebinding_to_the_same_actually_registered_chord_is_still_a_true_no_op() {
        // The mirror case: when `registered` really does hold the chord
        // being "rebound" to, the no-op optimization is correct and must
        // still apply — no wasted unregister/register round trip, and
        // `plan_apply`'s own coverage (`tests/shortcut_apply.rs`) already
        // proves this at the planning level; this proves it holds through
        // `apply_shortcut_with`'s use of `registered` as `old` too.
        let registrar = FakeRegistrar::new();
        let registered = Mutex::new(Some("CmdOrCtrl+Shift+R".to_string()));

        let result = apply_shortcut_with(&registrar, &registered, "CmdOrCtrl+Shift+R");

        assert!(result.is_ok());
        assert!(registrar.register_calls.lock().unwrap().is_empty());
        assert!(registrar.unregister_calls.lock().unwrap().is_empty());
        assert_eq!(
            registered.lock().unwrap().as_deref(),
            Some("CmdOrCtrl+Shift+R")
        );
    }

    #[test]
    fn rebinding_to_a_different_chord_updates_the_tracked_state() {
        let registrar = FakeRegistrar::new();
        let registered = Mutex::new(Some("CmdOrCtrl+Shift+R".to_string()));

        let result = apply_shortcut_with(&registrar, &registered, "Alt+Shift+E");

        assert!(result.is_ok());
        assert_eq!(
            registrar.unregister_calls.lock().unwrap().as_slice(),
            ["CmdOrCtrl+Shift+R"]
        );
        assert_eq!(
            registrar.register_calls.lock().unwrap().as_slice(),
            ["Alt+Shift+E"]
        );
        assert_eq!(registered.lock().unwrap().as_deref(), Some("Alt+Shift+E"));
    }

    #[test]
    fn a_rejected_new_chord_rolls_back_and_the_tracked_state_stays_on_the_old_one() {
        let registrar = FakeRegistrar::failing(&["Alt+Shift+E"]);
        let registered = Mutex::new(Some("CmdOrCtrl+Shift+R".to_string()));

        let result = apply_shortcut_with(&registrar, &registered, "Alt+Shift+E");

        assert!(result.is_err());
        assert_eq!(
            registered.lock().unwrap().as_deref(),
            Some("CmdOrCtrl+Shift+R"),
            "rollback succeeded, so the OS (and the tracked state) is back \
             on the old chord"
        );
    }

    #[test]
    fn a_media_key_accelerator_never_reaches_register() {
        // The startup path, which is the one that was exposed: a
        // hand-edited settings.json is handed to `register()` verbatim,
        // never passing through `Chord::to_accelerator` (whose own media
        // denylist is what `tests/shortcut.rs` covers, and which was never
        // the risk). Registering a media key routes into
        // `start_watching_media_keys` -> `CGEventTapCreate`, and creating
        // that session-level tap is what makes macOS demand Accessibility
        // / Input Monitoring — which this app must never do.
        //
        // Case variants and the `MediaTrackPrev` alias are included
        // because the plugin's own parser uppercases before matching and
        // accepts both spellings (global-hotkey-0.8.0 hotkey.rs), so an
        // exact-case check would let these through to the tap.
        for accel in [
            "CmdOrCtrl+MediaPlayPause",
            "cmdorctrl+mediaplaypause",
            "CmdOrCtrl+MEDIATRACKNEXT",
            "CmdOrCtrl+MediaTrackPrev",
            "CmdOrCtrl+MediaTrackPrevious",
            "Alt+MediaFastForward",
            "Alt+mediarewind",
        ] {
            let registrar = FakeRegistrar::new();
            let registered: Mutex<Option<String>> = Mutex::new(None);

            let result = apply_shortcut_with(&registrar, &registered, accel);

            assert!(result.is_err(), "{accel} must be refused");
            assert!(
                registrar.register_calls.lock().unwrap().is_empty(),
                "{accel} must never reach register() — that call is what \
                 creates the CGEventTap"
            );
            assert_eq!(registered.lock().unwrap().as_deref(), None);
        }
    }

    #[test]
    fn a_rejected_new_chord_whose_rollback_also_fails_leaves_the_tracked_state_empty() {
        let registrar = FakeRegistrar::failing(&["Alt+Shift+E", "CmdOrCtrl+Shift+R"]);
        let registered = Mutex::new(Some("CmdOrCtrl+Shift+R".to_string()));

        let result = apply_shortcut_with(&registrar, &registered, "Alt+Shift+E");

        assert!(result.is_err());
        assert_eq!(
            registered.lock().unwrap().as_deref(),
            None,
            "both the new chord and the rollback failed — the OS genuinely \
             has nothing registered, and the tracked state must say so"
        );
    }
}
