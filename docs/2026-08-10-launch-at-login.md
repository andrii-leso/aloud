# Launch at login — spike, then implementation

2026-08-10. Branch `feat/launch-at-login`, off `main` at `539bda4`.

`docs/HANDOFF.md` filed launch-at-login as a **spike, not a task**: three
unknowns stacked on it, each capable of invalidating the design. This
records the spike (run against the real installed bundle on this machine)
and the feature built on its result.

Machine: M1 MacBook Air, **macOS 26.6 (25G72)**. Every number below is from
that machine on that date; none of it is inherited from documentation.

---

## The spike

Run from inside `/Applications/Aloud.app` itself, because
`SMAppService.mainAppService` resolves through `NSBundle.mainBundle` — a
loose `target/release/aloud` asks about a directory that is not an app and
answers nothing useful. Two throwaway hooks were added to `main()`, the
bundle was built and signed by the ordinary `packaging/make-app.sh` path
(so: the real `com.andriileso.aloud` identifier, the real "Aloud Dev"
signature, the real `/Applications` path), and both hooks were removed in
the follow-up commit. The spike commit is `5ca89d8`, kept in history so the
result is reproducible.

### Unknown 1 — does `SMAppService.mainApp.register()` accept a self-signed, non-notarized bundle?

**Yes.** Verbatim output, twice, from `/Applications/Aloud.app/Contents/MacOS/aloud`:

```
exe:    Ok("/Applications/Aloud.app/Contents/MacOS/aloud")
before: NotFound
register: Ok
after register: Enabled
unregister: Ok
after unregister: NotRegistered
```

No error, no `BTMErrorDomain -98`, no `failed to construct identifier`, no
`kSMErrorInvalidSignature`. The bundle BTM accepted has
`TeamIdentifier=not set`, no Apple anchor, and
`designated => identifier "com.andriileso.aloud" and certificate leaf = H"e83bf2a9…"`.

This settles the open question in `M4-platform-research-macos.md` §2 in
favour of the TN3127 reading (BTM keys on a **stable, non-ad-hoc**
designated requirement) rather than the DTS "Apple-issued identity" reply
— which, as that research already flagged, was about an *embedded helper +
LaunchDaemon*, not `mainApp`. Aloud's self-signed cert is load-bearing
here for a second reason now, not just TCC: ad-hoc signing has no stable
DR, and that is the case DTS was actually describing.

Two secondary observations, both worth knowing before trusting the docs:

- **`register()` on an already-`Enabled` service returned `Ok`, not
  `kSMErrorAlreadyRegistered`.** Apple documents the error; macOS 26.6 did
  not produce it. The tolerant branch in `login_item::apply` is therefore
  defensive against documented-but-unobserved behaviour, not a path this
  machine exercises.
- **`unregister()` leaves the status at `NotRegistered`, not `NotFound`.**
  The record persists, disabled — which matches Apple's "the state is
  persisted to preserve user intent". `unregister()` *is* the API's own
  undo; `sfltool resetbtm` + reboot would be needed to erase the record
  entirely, and that was judged disproportionate to the residue. **The
  machine was left with the login item off** (confirmed by the app's own
  startup log line, below).

### Unknown 2 — does the `NSServices` selection path survive?

**Yes under a LaunchServices launch, which is what a login-item launch
is. The final confirmation still needs one logout/login — see "What needs
the owner's hands".**

Three pieces of evidence:

1. **The Service was invoked programmatically and worked**, via
   `NSPerformService("Read Aloud", pboard)` — the exact call the Services
   menu makes, requiring no Accessibility permission and no synthetic
   keystrokes. Returned `true`, and the app's own log shows the round trip:
   ```
   selection: Service callback fired, pasteboard text length=45 chars
   selection flow: detected language=en, normalized length=45 chars
   selection flow: speak started
   ```
   The app under test had been started with `open /Applications/Aloud.app`
   — i.e. by LaunchServices.
2. **The provider endpoint is registered in the user's launchd domain**:
   `launchctl print gui/501` shows `com.andriileso.aloud.ServiceProvider`.
3. **The running app appears as a LaunchServices *application*, not a
   plain agent**: its launchd label is
   `application.com.andriileso.aloud.413710448.413710454`. That
   `application.<bundle-id>.*` form is what LaunchServices produces.
   `SMAppService.mainApp` registers the **bundle**, and Apple's contract is
   "the application launches on subsequent logins" — the same path.

This is the fork that killed `tauri-plugin-autostart`: its default
LaunchAgent mode writes `ProgramArguments` pointing at
`Contents/MacOS/aloud`, launching the inner executable directly and
bypassing LaunchServices entirely. `SMAppService` does not do that, which
is the whole reason it was chosen.

### Unknown 3 — does `tao` cause a visible focus grab at login?

**No, not observably.** `tao` 0.35.3 does call it unconditionally —
`AppState::launched` (`app_state.rs:293`) runs
`ns_app.activateIgnoringOtherApps(ignore)` with `ignore` defaulting to
`true` — but it runs *after* `apply_activation_policy`, and Aloud is
`ActivationPolicy::Accessory` with no window at launch. There is nothing
to bring forward.

Measured rather than argued: the frontmost app was sampled every 500 ms
for 10 s across a launch (`lsappinfo front`, no permission required).

```
before: "LSDisplayName"="Claude"
  20 "LSDisplayName"="Claude"      # 20/20 samples, spanning the launch
```

Frontmost never changed. tauri#15017 is real but moot here.

---

## What was built

Spike green on all three, so the feature shipped in the same branch.

| File | What |
|---|---|
| `src/login_item/mod.rs` | Status model (`LoginItemStatus`), the `LoginItemService` seam, and `apply()` — the parts that are not Objective-C interop, so they are unit-testable |
| `src/login_item/macos.rs` | The `SMAppService` binding: `status` / `register` / `unregister` / `openSystemSettingsLoginItems` |
| `src/settings.rs` | `launch_at_login: bool`, default **false** |
| `src/bin/aloud.rs` | `get_login_item_status`, `set_launch_at_login`, `open_login_items_settings`; a startup status read |
| `dist/` | The "Launch at Login" section |
| `Cargo.toml` | `objc2-service-management` 0.3.2 |

### The dependency

`objc2-service-management` 0.3.2, `default-features = false`, features
`std`, `objc2`, `objc2-foundation`, `SMAppService`. It is the same objc2
**0.3.x** generation the crate already uses for AppKit and Foundation, so
it adds no second bindings generation, no Swift helper, and no build
script. The alternatives were `smappservice-rs` (a wrapper over the same
thing) or hand-rolling four `msg_send!`s plus a
`#[link(name = "ServiceManagement", kind = "framework")]` — the crate is
smaller than the hand-roll and does not need auditing for selector typos.

`objc2-foundation` gained the `NSError` feature, named explicitly rather
than inherited from the new crate's own feature set: this crate names what
it uses.

**Not `tauri-plugin-autostart`** — see unknown 2, `CLAUDE.md`, and
`M4-platform-research-macos.md` §2.

### The design decision that matters: reality beats intent

`launch_at_login` in `settings.json` is **not** the source of truth for
whether Aloud launches at login. It cannot be: the real state lives in
macOS's Background Task Management store, outside the app, and the user
can switch it off in System Settings → General → Login Items **without
Aloud ever being told**. Apple's own `Status` docs say so: `requiresApproval`
is returned "if the user revokes consent for the service to run in System
Settings".

So:

- The settings window renders `SMAppService.mainApp.status`, read live on
  every window load and after every change. `LoginItemStatus::is_on()` is
  true for **`Enabled` only** — a user who switched Aloud off in System
  Settings sees the toggle off.
- `set_launch_at_login` returns the OS's **read-back status**, never the
  request. `register()` returning `Ok` is not proof the app will launch:
  if consent was previously revoked the real status is `RequiresApproval`,
  and the window says so and offers the Login Items button (the only thing
  that can actually fix it — re-registering cannot grant consent).
- Ordering is `apply → persist`, the same as `set_shortcut`. Nothing
  reaches disk until the OS has accepted.
- Startup **reads and reports, never writes**. If the saved bool and the
  live status disagree, the log says so and nothing is re-registered —
  because the commonest cause of disagreement is a deliberate opt-out that
  Aloud was not notified of. `M4-platform-research-macos.md` §2 finding 3:
  "never re-`register()` to 'fix' a deliberate opt-out."

What the persisted bool *is* for: a record of intent, so the divergence
above can be noticed and said out loud rather than silently papered over.

### Settings window height

The window is fixed at 480 px and **not resizable**, so a new section can
push content below a fold nobody can scroll past comfortably. Measured in
a browser at 480 px: the page is **618 px** tall in its ordinary state and
**710 px** with the `RequiresApproval` note and its Login Items button
showing. `height` went 560 → **700** in both `tauri.conf.json` and
`open_settings_window`'s builder fallback (the two must agree; the builder
branch only runs if the config-created window was destroyed).

---

## Tests

`cargo test --release`: **157 passed, 0 failed, 2 ignored** (the two
pre-existing `#[ignore]`d ones — the absolute-latency check and the
ASR-backed `speed_preserves_words`).

New coverage, all at layers that do not need a live `SMAppService`:

- `src/login_item/mod.rs` — 10 tests: the raw-status mapping; `is_on` true
  for `Enabled` only; `apply` returning the read-back status rather than
  the request, including the case where `register()` succeeds but the OS
  reports `RequiresApproval`; the two idempotence codes folded into
  success, and **not** swapped between directions; a real failure
  propagating without reading back a status.
- `src/bin/aloud.rs` `command_tests` — 3 tests through the real IPC
  pipeline under `MockRuntime`, with a fake `LoginItemService` in managed
  state: the toggle reading live status rather than the saved bool; a
  refused registration erroring **and leaving disk untouched**; an accepted
  one persisting and reporting the read-back status. These two commands are
  reachable this way (unlike `set_shortcut`/`set_voice`/`set_speed`)
  because they take only `State`, never `AppHandle`.
- `tests/settings.rs` — the round-trip carries the new field, the default
  is asserted **off**, and a `settings.json` written before the field
  existed still loads with its shortcut, voice and speed intact.

**Not unit-testable, by nature:** the `SMAppService` calls themselves.
`mainAppService` resolves through `NSBundle.mainBundle`, so it is
meaningless from a test binary — which is precisely why the seam exists.
Those calls were verified empirically instead, by the spike above. No test
pretends to cover them.

---

## What needs the owner's hands

`computer-use` cannot reach a Dock-less accessory app, and enabling
Accessibility to work around that is a standing project refusal. So:

1. **One logout and login back in, with the toggle on.** This is the only
   remaining gap in unknown 2. Turn "Launch Aloud at login" on in Settings,
   log out, log back in, then confirm (a) Aloud's menu-bar icon is there,
   and (b) **⌘⇧A still reads a selection**. Everything short of the actual
   logout says it will work; nothing short of it proves it.
2. **Turn it off again afterwards if you don't want it.** It ships default
   off and this session left it off — turning it on is entirely your call.
3. **Eyeball the new section.** The toggle, the "Aloud will start at
   login." confirmation, and — if macOS ever reports `RequiresApproval` —
   the note plus the "Open Login Items…" button. The layout was measured in
   a browser, not seen in the real window.
4. The settings window is now 700 px tall rather than 560. If that feels
   wrong on this display, say so; it was sized to keep a failure message
   above the fold.

## Undoing this entirely

`unregister()` (i.e. switching the toggle off) is the supported undo and
leaves the status at `NotRegistered`. macOS keeps a disabled record; to
erase that too, `sfltool resetbtm` followed by a reboot — which resets
**every** app's background-item state on this machine, not just Aloud's.
It was not run here.
