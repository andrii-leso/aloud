# Closing the settings window quit the whole app — fix, 2026-08-10

Andrii clicked the red X on the Settings window. The tray icon disappeared —
the whole app had quit. He recovered by pressing `⌘⇧A` (the macOS Service),
which relaunched the bundle via LaunchServices.

## Confirmed mechanism

**Tauri's default `RunEvent` handling exits the process the instant the last
open window is destroyed, and `aloud.rs` never overrode it.**

`main()` ended in `.run(tauri::generate_context!())` — `Builder::run(context)`,
which tauri's own source (`tauri-2.11.5/src/app.rs:2449`) defines as sugar for:

```rust
self.build(context)?.run(|_, _| {});
```

A no-op callback. Tracing the runtime (`tauri-runtime-wry-2.11.4/src/lib.rs`):

1. `TaoWindowEvent::CloseRequested` → `on_close_requested()` fires
   `WindowEvent::CloseRequested` to the window's own `on_window_event`
   listeners (aloud's two closures — see "Ruling out the second candidate"
   below) and to the app-level `.run()` callback. Neither called
   `api.prevent_close()`, so the window proceeds to close.
2. `TaoWindowEvent::Destroyed` fires. The runtime removes the window from
   its tracked set and checks `windows.is_empty()` (line 4313). For Aloud,
   the settings window is the **only** window Tauri tracks — the tray icon
   isn't one — so this is true.
3. Because it's empty, the runtime fires `RunEvent::ExitRequested { code:
   None, tx }` (line 4316). The no-op callback never reads `tx`, so
   `rx.try_recv()` returns `Err`, `should_prevent` is `false`, and
   `control_flow = ControlFlow::Exit` (line 4322) — the whole process exits.

This is generic Tauri behavior for any app that never supplies a `RunEvent`
callback; nothing Aloud-specific is required to trigger it. Tauri's own
`App::run` doc comment (`app.rs:1359-1364`) shows the fix as a documented
pattern: build the app, then call `.run(|_, event| if let
RunEvent::ExitRequested { api, .. } = event { api.prevent_exit(); })`.

### Ruling out the second candidate

The window-close handlers in `aloud.rs` (`open_settings_window`'s builder
branch and the matching closure in `setup()` for the config-declared window)
only do two things on `CloseRequested`: `let _ =
app.set_activation_policy(Accessory)` (return value discarded, cannot panic)
and `disarm_probe(reason)` (an `AtomicBool::store` plus a log line — no panic
surface). Neither can tear down the tray or crash the process. The live
reproduction below confirms this directly: both closures completed and
logged `"probe: disarmed (settings window closed)"` successfully, in both
the broken and fixed builds — the crash always happened *after* that log
line, in the runtime's own last-window-destroyed handling, not inside
Aloud's handler.

### Evidence from the actual incident

`~/Library/Logs/Aloud/aloud.log` from Andrii's real session:

```
[1786371549] selection flow: speak started
[1786371561] probe: disarmed (settings window closed)
[1786371561] speak_selection: error: Non-zero status code ... (unrelated ONNX error, same second by coincidence)
[1786371626] app start          <- 65s later: a FRESH relaunch, not a resumed process
[1786371628] engine load complete
[1786371628] hotkey: registered CmdOrCtrl+Shift+R
```

The settings-window-closed log line is followed by total silence, then a
brand new `app start` / `engine load complete` sequence — a relaunch, exactly
as the mechanism above predicts, not a process that kept running.

### Live reproduction (before the fix)

Built `--release`, stopped the then-running production instance (pid 22873,
so this test wouldn't collide with it), and ran the pre-fix binary with a
temporary, env-var-gated harness that opens and closes the settings window
via real Rust calls (`open_settings_window` / `WebviewWindow::close()`) — no
Accessibility, no synthetic keystrokes:

```
t+9s:  alive
t+10s: DEAD
=== log ===
[..] repro: opening settings window
[..] settings: re-showed existing window
[..] repro: calling window.close() programmatically
[..] repro: close() call returned
[..] probe: disarmed (settings window closed)
```

The harness's own next log line (`"repro: still alive 3s after close()
returned"`) never printed — the process died within about a second of
`close()` returning, before that statement could execute. `pgrep -x aloud`
confirmed the process was gone.

## The fix

`src/bin/aloud.rs`: `main()` now builds the app and supplies a real
`RunEvent` callback instead of relying on `Builder::run`'s no-op default:

```rust
.build(tauri::generate_context!())
.expect("error while running Aloud")
.run(|_app_handle, event| {
    if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
        if should_prevent_exit(code) {
            api.prevent_exit();
        }
    }
});
```

```rust
fn should_prevent_exit(code: Option<i32>) -> bool {
    code.is_none()
}
```

`RunEvent::ExitRequested`'s own doc comment gives the discriminator:
`code` is `None` when the runtime is asking to exit because of user
interaction (the last-window-destroyed path above), and `Some(_)` when
something called `AppHandle::exit()` or `AppHandle::restart()` — which is
exactly what the tray's "Quit Aloud" item does (`app.exit(0)` at
`aloud.rs:850`, unchanged). Only the `None` case calls `prevent_exit()`. A
blanket `prevent_exit()` (the shape of Tauri's own doc example) would have
also swallowed the deliberate quit — that was checked explicitly, see below.

This is the root-cause fix, not a workaround: it supplies the callback Tauri
expects a long-lived tray app to supply, rather than special-casing the
window's close behavior to route around a runtime default that doesn't fit
this app's shape.

## What was verified, and how

All of the following were exercised against the real (non-mock, non-test)
release binary, via real Rust API calls from a background thread — never a
click, never Accessibility, never a synthetic keystroke:

| Requirement | Verified | Method |
|---|---|---|
| Closing settings leaves the app running, tray intact | Yes | `pgrep -x aloud` stayed alive through and past two separate opens+closes |
| Quit (`app.exit(0)`) still exits cleanly | Yes | Called `AppHandle::exit(0)` programmatically (the exact call the tray's "Quit Aloud" item makes) after two open/close cycles; process exited within ~1s |
| Reopening settings after closing it works (builder branch) | Yes | Log shows `"settings: created window"` on the second open — the config-declared window was destroyed by the first close, so the builder branch (not the re-show branch) ran, and it worked |
| Activation-policy restore still happens (no lingering Dock icon) | Yes, by inspection + indirect evidence | The two `on_window_event` closures that call `set_activation_policy(Accessory)` are byte-for-byte unchanged by this fix; both were observed completing without error (`"probe: disarmed (settings window closed)"` logged both times). A fresh production launch was also confirmed `background only: true` via `osascript`/System Events (Accessory policy, no Dock icon) |
| `⌘⇧R` (region hotkey) still works after a close | Verified registration only | Log confirms `"hotkey: registered CmdOrCtrl+Shift+R"` on every launch, unaffected by this diff (hotkey registration code is untouched and runs once at startup, independent of the run-loop change) |
| `⌘⇧A` (Service) still works after a close | **Not verified — manual check needed** | The Service dispatch path (`register_service_provider`) is untouched by this diff, but actually firing it requires a real text selection + the macOS Service menu, which is exactly the UI-driving this task ruled out |

### Live reproduction (after the fix)

Same harness pattern, this time: open → close → reopen (forces the builder
branch) → close again → call `app_handle.exit(0)` (the real Quit path):

```
t+1..12s: alive
t+13s:    DEAD
=== log ===
[..] repro: opening settings (1st time)
[..] settings: re-showed existing window
[..] repro: closing settings (1st close)
[..] probe: disarmed (settings window closed)
[..] repro: still alive after 1st close; reopening (2nd time)
[..] settings: created window          <- builder branch, as expected
[..] repro: closing settings (2nd close)
[..] probe: disarmed (settings window closed)
[..] repro: still alive after 2nd close; calling app_handle.exit(0) now
=== pgrep after ===
(empty — process exited cleanly after the deliberate exit(0))
```

Both temporary harnesses (the "before" reproduction and the "after"
verification) were removed before the final build — `git diff` against
`main` touches only `src/bin/aloud.rs`'s `main()` and adds the
`should_prevent_exit` helper plus its tests, nothing else.

## Automated regression test

Added to `src/bin/aloud.rs`, `mod run_event_tests`:

1. **`window_close_is_prevented_but_a_coded_exit_is_not`** — a plain unit
   test of `should_prevent_exit`'s discrimination: `None → true`, `Some(0) →
   false`. Fast, deterministic, no runtime involved.
2. **`closing_the_only_window_does_not_end_the_run_loop`** — an integration
   test through the *real* `Builder → build → run` wiring (Tauri's
   `MockRuntime`, not the production wry/tao runtime, but the same code
   path aloud's `main()` exercises): builds a mock app with one window,
   registers the exact same callback logic, closes the window from the
   test thread, and asserts `app.run()` does **not** return within 3
   seconds (which would mean the loop exited). This is a real regression
   test — running it against the pre-fix code (bare `.run(context)`, no
   callback) would fail, because the mock runtime's own `run()`
   implementation (`tauri-2.11.5/src/test/mock_runtime.rs:1327-1394`)
   reproduces the identical "last window destroyed → unprevented
   `ExitRequested` → loop breaks" logic as the real wry runtime.

**What could not be covered by an automated test, and why:** the "a
deliberate quit must still exit cleanly" half. `MockRuntimeHandle::
request_exit` — what `AppHandle::exit()` calls under the hood, i.e. exactly
what the tray's "Quit Aloud" item triggers — is `unimplemented!()` in tauri
2.11.5's test runtime (`mock_runtime.rs:143-145`); calling it panics. There
is no way to drive that path through `MockRuntime` at all. This half was
verified by hand against the real release binary instead (see "Live
reproduction (after the fix)" above) and has no regression coverage at this
layer. **If `main()`'s run-loop callback is touched again, re-run that
manual check** (`app_handle.exit(0)` from a background thread, or an
equivalent), since a regression here (e.g. someone "simplifying" the
predicate to an unconditional `prevent_exit()`) would silently break Quit
and nothing in `cargo test` would catch it.

## Test suite

Full default suite, `--release` (per hard constraint 10): **143 passed, 0
failed, 2 ignored** (the two pre-existing `#[ignore]`d expensive/
load-sensitive tests — `absolute_time_to_first_audio_budget` and
`raising_the_speed_does_not_drop_words` — unrelated to this change, per
existing hard constraints 5/6). One incidental flake was observed on a
single run of `engine_speed_pin` (TTS timing test, unrelated to this diff)
under parallel load from a prior background `tee`; it passed cleanly on
every other run, consistent with the documented load-dependent timing
behavior of the TTS engine (CLAUDE.md hard constraint 5), not a regression
from this change.

## Build and disk

`packaging/make-app.sh` ran clean — the "Aloud Dev" self-signed certificate
was present (`security find-certificate -c "Aloud Dev"` found it; an
earlier `security find-identity -v` check was a red herring — that command
filters to *trusted* identities, and this cert is deliberately self-signed
and untrusted, which the script's own comments say is expected and fine).
Disk: 12 Gi used / 17 Gi available before and after (`df -h /`), unchanged.

## Owner's settings

`~/Library/Application Support/com.andriileso.aloud/settings.json` was never
written to during this work — confirmed by grepping the log for
`"settings: saved"` events after testing began (none). Current contents:

```json
{
  "region_shortcut": "CmdOrCtrl+Shift+R",
  "voice": "F5",
  "speed": 1.5
}
```

**Note:** the task brief expected `voice: M5`; the file actually has `F5`.
The log shows this is not something this session touched — `voice: switched
to F5` was logged at `[1786371452]`, which is *before* the crash itself
(`[1786371561]`) and well before any work in this session began
(`[1786372151]` onward). It looks like Andrii's own settings-window session
switched the voice shortly before hitting the bug. `region_shortcut` and
`speed` match the brief exactly; `voice` was left exactly as found, per the
instruction not to reset or rebind anything.

## What the owner should re-check by hand

1. **`⌘⇧A` (the Service) after closing Settings once.** Not exercised in
   this session (would require driving the Service via a real text
   selection, which is UI-driving this task ruled out). Expected to work —
   `register_service_provider` is untouched — but worth one real click to
   be sure, since it's the exact recovery path that got used last time.
2. **The actual red-X click**, if you want the literal original repro.
   Every mechanism behind it was exercised programmatically and it behaved
   correctly every time, but nothing in this session literally clicked the
   window chrome.
3. **No Dock icon appears** after opening and closing Settings during
   normal use. Verified once via `osascript`/System Events
   (`background only: true`) on a fresh launch with no window ever opened,
   and the restore-on-close code path is unchanged and was observed
   completing without error twice — but a direct look at the Dock after a
   real open/close is the strongest confirmation.
