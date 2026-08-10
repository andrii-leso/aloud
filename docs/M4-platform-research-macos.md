# M4 platform research — macOS

What **macOS itself** requires of the M4 surface: a menubar app that gains a settings
window, launch-at-login, a global-shortcut rebinding UI, and packaging/distribution.

Written 2026-08-09, before the M4 plan. This exists because M3 was planned by verifying
every Rust *crate* API from source and doing **no** research into what the operating
system requires — and the owner's first real use hit four bugs that 80 passing tests
missed, three of them documented platform requirements findable in one search
(`M3-carry-forward.md` §"First-use findings"). This document closes that gap for M4.

**Target platform:** macOS **26.6** (build 25G72), the machine Aloud ships on.
**Bundle under test:** `/Applications/Aloud.app`, hand-assembled by `packaging/make-app.sh`,
self-signed "Aloud Dev", not notarized.

Confidence tags on every finding:
- **VERIFIED** — read in a primary source; citation given (URL, `gh` command, or absolute file path).
- **LIKELY** — strong secondary evidence, named.
- **UNVERIFIED** — could not confirm; what would confirm it is stated.

Apple's documentation site is JS-rendered and defeats a plain fetch. Every Apple doc
below was read through its own backing JSON API
(`https://developer.apple.com/tutorials/data/documentation/<path>.json`) — the
web-fetching doctrine's §5 rung 3, "find the internal API". Use that, not a scraper.

---

## 1. Menubar app with a settings window

### `LSUIElement` and the accessory activation policy are the same state

- **VERIFIED** — `NSApplication.ActivationPolicy.accessory`: *"The application doesn't
  appear in the Dock and doesn't have a menu bar, but it may be activated
  programmatically or by clicking on one of its windows."* Its Discussion adds: *"This
  corresponds to value of the `LSUIElement` key in the application's `Info.plist` being
  `1`."*
  https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/accessory
- **VERIFIED** — So accessory policy removes the Dock icon and the app menu bar, and
  **nothing else**. It does not prevent windows, and it explicitly permits activation.
  The policy that forbids windows is a different one — `.prohibited`: *"The application
  doesn't appear in the Dock and may not create windows or be activated"* (= `LSBackgroundOnly`).
  Aloud must never be set to `.prohibited`.
  https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/prohibited
- **VERIFIED** — `.regular` is *"the default for bundled apps, unless overridden in the
  `Info.plist`"*. This is why a Tauri app shows a Dock icon from a bundle but not from a
  cargo-run binary — LaunchServices applies `Info.plist`, a terminal launch does not.
  https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/regular
- **VERIFIED** — `setActivationPolicy(_:)`: *"You can set any activation policy in macOS
  10.9 and later"* — runtime switching Regular↔Accessory is supported.
  https://developer.apple.com/documentation/appkit/nsapplication/setactivationpolicy(_:)

Aloud today sets **both**: `LSUIElement=true` in the bundle plist
(`packaging/make-app.sh`) and `app.set_activation_policy(ActivationPolicy::Accessory)`
in `setup` (`src/bin/aloud.rs:157`). That belt-and-braces is correct and should stay —
`tao` deliberately defers the policy until after launch (its own comment: *"If the
activation policy is set earlier, the menubar is initially unresponsive on macOS 10.15
for example"*, `~/.cargo/registry/.../tao-0.35.3/src/platform_impl/macos/app_delegate.rs:31-33`),
so the plist key is what covers the window between process start and `applicationDidFinishLaunching`.

### The real gate on keyboard input is `canBecomeKey`, not the activation policy

- **VERIFIED** — `NSWindow.canBecomeKey`: *"The value of this property is true if the
  window has a title bar or a resize bar, or false otherwise."* and *"Attempts to make
  the window the key window are abandoned if the value of this property is false."*
  A borderless window **silently refuses keyboard input** regardless of activation policy.
  https://developer.apple.com/documentation/appkit/nswindow/canbecomekey
- **VERIFIED** — Tauri's `WindowConfig` defaults `decorations: true`, `visible: true`,
  `focus: true`, `focusable: true`
  (`~/.cargo/registry/.../tauri-utils-2.9.3/src/config.rs:2318-2323`). A default Tauri
  window therefore has a title bar and *can* become key. Setting `decorations: false`
  for a "clean" settings panel would break text entry.
- **VERIFIED** — `NSPanel` is the wrong shape here: `canBecomeMain` is false for any
  `NSPanel`, and `hidesOnDeactivate` *"default value for `NSWindow` is false; the default
  value for `NSPanel` is true"*.
  https://developer.apple.com/documentation/appkit/nswindow/canbecomemain ·
  https://developer.apple.com/documentation/appkit/nswindow/hidesondeactivate
- **VERIFIED** — `NSPopover.Behavior.transient` is also wrong: Apple explicitly disclaims
  its dismissal rules — *"The exact interactions that will cause transient popovers to
  close are not specified."*
  https://developer.apple.com/documentation/appkit/nspopover/behavior-swift.enum/transient
- **VERIFIED** — `NSStatusItem`'s documentation contains **no** guidance on window
  activation or focus. There is no Apple-blessed "status item opens a settings window"
  recipe to follow.
  https://developer.apple.com/documentation/appkit/nsstatusitem

### Bringing the window forward: activation is a request, not a command

- **VERIFIED** — `activate(ignoringOtherApps:)` is **not yet hard-deprecated on macOS 26**.
  Apple's own metadata: `deprecated: false`, `deprecatedAt: "27.0"`, `introducedAt: "10.0"`,
  message verbatim: *"This method will be deprecated in a future release. Use NSApp.activate
  instead."*
  https://developer.apple.com/documentation/appkit/nsapplication/activate(ignoringotherapps:)
  (fetched via the doc JSON API and independently reproduced twice.)
- **VERIFIED** — But **its argument has been inert since macOS 14.** WWDC23 session 10054,
  verbatim: *"Now that activate is a request, the ignoringOtherApps parameter and option
  are ignored."* https://developer.apple.com/videos/play/wwdc2023/10054/
- ⚠ **Documented tension, worth naming:** WWDC23 says the API is "deprecated in macOS
  Sonoma"; the current headers say "will be deprecated" at 27.0. Both agree the behaviour
  already degraded. Treat it as functionally dead now, removed at macOS 27.
- **VERIFIED** — Replacement `NSApplication.activate()` (macOS 14+): *"Use this method to
  request app activation; calling this method doesn't guarantee app activation."*
  https://developer.apple.com/documentation/appkit/nsapplication/activate()
  Cooperative-activation model: *"Cooperative activation addresses this problem by making
  app activation a request instead of a command… The system dynamically determines whether
  to grant activation state based on the context."*
  https://developer.apple.com/documentation/appkit/passing-control-from-one-app-to-another-with-cooperative-activation
- **VERIFIED** — `orderFrontRegardless()`: *"Moves the window to the front of its level,
  even if its application isn't active, **without changing either the key window or the
  main window**."* It brings a window forward and grants **no** keyboard focus. Do not use
  it for the settings window.
  https://developer.apple.com/documentation/appkit/nswindow/orderfrontregardless()
- **VERIFIED** — `makeKeyAndOrderFront(_:)` alone is also insufficient. From
  `activate(ignoringOtherApps:)`'s own Discussion: *"When you send a `makeKey()` message
  to an `NSWindow` object, you ensure that it's the key window **when the app is active**."*
  Activation and key-status are two separate steps.
- **UNVERIFIED** — Apple documents **no** special case for what `NSApp.activate()` does for
  an accessory-policy app. The only relevant sentence is that an accessory app "may be
  activated programmatically"; whether the system *grants* the request is context-dependent
  and undocumented. Confirming this needs an empirical check on 26.6: after showing the
  window, read `NSApp.isActive` and `window.isKeyWindow`.

### What Tauri/tao actually do — read from the exact versions Aloud compiles

- **VERIFIED (silent-failure trap)** — `tao::Window::set_focus()` is a **no-op on a
  non-visible window**, with no error and no log:
  ```rust
  pub fn set_focus(&self) {
    unsafe {
      let is_minimized = self.ns_window.isMiniaturized();
      let is_visible = self.ns_window.isVisible();
      if !is_minimized && is_visible {
        util::set_focus(&self.ns_window);
      }
    }
  }
  ```
  `~/.cargo/registry/src/index.crates.io-.../tao-0.35.3/src/platform_impl/macos/window.rs:677-685`.
  Calling Tauri's `set_focus()` before `show()` silently does nothing.
- **VERIFIED** — when it *does* run, `util::set_focus` performs exactly the right pair:
  `ns_window.makeKeyAndOrderFront(None)` followed by
  `msg_send![app, activateIgnoringOtherApps: YES]` —
  `tao-0.35.3/src/platform_impl/macos/util/async.rs:231-238`. So `show()` then
  `set_focus()` is the correct Tauri sequence; the activation half is the soft-deprecated
  call, which on macOS 14+ degrades to a plain `activate()` request.
- **VERIFIED** — `tao`'s launch-time `window_activation_hack` **explicitly skips invisible
  windows** (`"Skipping activating invisible window"`), so a `visible: false` startup window
  gets no launch-time activation help.
  https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/macos/app_state.rs
- **VERIFIED** — `AppHandle::set_activation_policy(&self) -> Result<()>` exists (needed to
  call it from a tray handler, not just `setup`), added by tauri PR **#9842**, merged
  2024-05-21. `~/.cargo/registry/.../tauri-2.11.5/src/app.rs:640`.
- **VERIFIED (tray behaviour that shapes the UI)** — `TrayIconBuilder::show_menu_on_left_click`
  defaults to **`true`** (`tauri-2.11.5/src/tray/mod.rs:300-322`), and Aloud sets it
  explicitly (`src/bin/aloud.rs:195`). A left click therefore opens the tray menu; it does
  not deliver a usable click event for "open settings". Open the settings window from a
  **menu item**, not from a tray click, unless you are willing to give up the menu.

**Real, currently-open Tauri issues on this exact surface** (verified via `gh`, numbers and
dates as returned — none invented):

| # | State | Opened | Title | Relevance |
|---|---|---|---|---|
| [15005](https://github.com/tauri-apps/tauri/issues/15005) | open | 2026-02-26 | macOS: Dock icon visible when app installed from .app bundle, but not in dev mode (menu-bar-only app) | Maintainer could not reproduce; a second reporter (2026-05-30) adds *"macos is doing its level best to place the window behind every other window"* — directly the M4 settings-window risk |
| [12128](https://github.com/tauri-apps/tauri/issues/12128) | open | 2025-01-02 | [bug] open app from deep link will break accessory activation policy | Re-shows the Dock icon; workaround re-calls `set_activation_policy` after a visible flash |
| [15017](https://github.com/tauri-apps/tauri/issues/15017) | open | 2026-03-01 | [feat] Expose set_activate_ignoring_other_apps from tao on macOS | See §5 — the launch-at-login focus steal |
| [6781](https://github.com/tauri-apps/tauri/issues/6781) | open | 2023-04-24 | [macOS] acceptFirstMouse config doesn't always work, dependent on how window is toggled | Menubar app needs **two clicks** to focus a text input when the window is shown from a tray menu; reporter traced it to lost key-window status. Open 3 years |
| [9244](https://github.com/tauri-apps/tauri/issues/9244) | closed 2026-07-21 | 2024-03-21 | set_activation_policy is not accessible during runtime | Fixed by #9842 |

- **UNVERIFIED** — no open issue reports that an accessory-policy Tauri 2 window fails to
  take keyboard input on macOS 26 specifically. Absence of evidence only.

### What this means for M4

1. **Ship the settings window with `decorations: true` (a real title bar).** `canBecomeKey`
   is false without a title or resize bar, so a chrome-less panel will silently refuse
   text input — and the shortcut-recording field is the whole point of the window.
2. **Order the calls `show()` → `set_focus()`, never the reverse.** `tao`'s `set_focus()`
   early-returns on a non-visible window with no error. This is a silent no-op, not a bug
   you will see in a log.
3. **Use a normal `NSWindow` (Tauri's default), not `NSPanel` or `NSPopover`.** `NSPanel`
   hides on deactivate and can never become main; `NSPopover.transient`'s dismissal rules
   are explicitly "not specified" by Apple. Both are wrong for a window the user tabs
   through and types into.
4. **Open the window from a tray *menu item*, and assert focus rather than assuming it.**
   Left-click is consumed by the menu (`show_menu_on_left_click` defaults true), and
   activation on macOS 14+ is a request the system may decline — issue #6781 is a live
   report of exactly this failing for a tray-shown text input.

---

## 2. Launch at login

### `SMAppService` is the current-generation API

- **VERIFIED** — `SMAppService`, macOS 13.0+: *"An object the framework uses to control
  helper executables that live inside an app's main bundle."*
  https://developer.apple.com/documentation/servicemanagement/smappservice
- **VERIFIED** — `SMAppService.mainApp`: *"An app service object that corresponds to the
  main application as a login item… Use this `SMAppService` to configure the main app to
  launch at login."*
  https://developer.apple.com/documentation/servicemanagement/smappservice/mainapp
- **VERIFIED** — `register() throws`: *"If the service corresponds to the main application,
  the application launches on subsequent logins."* Documented errors: *"If the service is
  already registered, this method returns `kSMErrorAlreadyRegistered`. If the service isn't
  approved by the user, this method returns `kSMErrorLaunchDeniedByUser`."*
  https://developer.apple.com/documentation/servicemanagement/smappservice/register()
- **VERIFIED** — `unregister() throws`: *"If the service corresponds to the main application,
  it continues running, but becomes unregistered to prevent future launches at login. If the
  service is already unregistered, this method returns `kSMErrorJobNotFound`."*
- **VERIFIED** — `SMAppService.Status`, verbatim abstracts:
  - `notRegistered` — *"The service hasn't registered with the Service Management framework,
    **or the service attempted to reregister after it was already registered**."*
  - `enabled` — *"The service has been successfully registered and is eligible to run."*
  - `requiresApproval` — *"successfully registered, but the user needs to take action in
    System Settings… **The framework also returns this status if the user revokes consent
    for the service to run in System Settings.**"*
  - `notFound` — *"An error occurred and the framework couldn't find this service."*
- **VERIFIED** — `SMAppService.openSystemSettingsLoginItems()` exists to deep-link the user
  to the pane; Apple's migration article tells you to call it when status is not authorized.
  https://developer.apple.com/documentation/servicemanagement/updating-helper-executables-from-earlier-versions-of-macos
- **VERIFIED** — user-visible state on macOS 26 is **System Settings → General → Login Items
  & Extensions**, with an "Open at Login" list and an "App Background Activity" section
  (renamed from Ventura's "Allow in the Background").
  https://support.apple.com/en-my/guide/mac-help/mtusr003/26/mac/26
  WWDC22 session 10096 ("What's new in privacy"), verbatim: *"Your app will be allowed to
  launch at login by default, and users will be notified."*
  https://developer.apple.com/videos/play/wwdc2022/10096/
- **VERIFIED (real failure shape)** — the common on-record failure is
  `Error Domain=SMAppServiceErrorDomain Code=1 "Operation not permitted"`, backed by the
  private `BTMErrorDomain`. Apple DTS, verbatim: *"`BTMErrorDomain` is a private error
  domain associated with the BTM ('background task management') subsystem that underlies
  the `SMAppService` API. Error -98 translates (I think :-) to 'invalid parameter', which
  isn't very helpful."* The failing log line is
  `failed to construct identifier with parameters: appURL=/Applications/…app, url=(null), type=app`.
  https://developer.apple.com/forums/thread/722338
- **UNVERIFIED** — Apple does not publish a mapping of `SMAppServiceErrorDomain` numeric
  codes. `Code=1 ↔ "Operation not permitted"` is only observable empirically.

### Does it work for a self-signed, non-notarized app? — the highest-value question

**There is no documented signing requirement. That is a verified absence, not a green light.**

- **VERIFIED by absence** — none of `SMAppService`, `mainApp`, `register()`, `Status`, or
  the ServiceManagement framework page states any code-signing, Developer ID, notarization,
  or `/Applications` requirement. The only signing-adjacent constant is
  `kSMErrorInvalidSignature`, whose abstract is generic: *"The app's code signature doesn't
  meet the requirements to perform the operation."*
- **VERIFIED (Apple DTS, verbatim)** — *"If you're seeing that it's possibly because your
  code isn't signed correctly. Specifically, you should be signing both your embedded helper
  and your app with the same Apple-issued code-signing identity. If, for example, you are
  using ad hoc signing (Sign to Run Locally in Xcode parlance) then you will see problems
  like this."* https://developer.apple.com/forums/thread/799910
  ⚠ Caveat that matters: that thread is about an **embedded helper + LaunchDaemon**, not
  `mainApp`, and the reporter was on a free Apple *development* profile — so the sentence
  argues for *Apple-issued* and says nothing about Developer ID or notarization.
- **VERIFIED (Apple TN3127, "Inside Code Signing: Requirements")** — the mechanism these
  subsystems key on is the **designated requirement**, and CA identity is not what makes it
  stable: *"Unsigned code has no DR. Ad hoc signed code, called Sign to Run Locally by
  Xcode, has a DR but it's tied to that specific version of the code. In both cases macOS
  can't reliably track the identity of the code."*
  https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements
  This is the *same* mechanism as the TCC-stability reason Aloud is self-signed — and Aloud
  is on the good side of it.
- **VERIFIED (measured on this machine, read-only)** — Aloud already satisfies the
  DR-stability condition DTS warns about:
  ```
  Identifier=com.andriileso.aloud
  Authority=Aloud Dev
  TeamIdentifier=not set
  designated => identifier "com.andriileso.aloud" and certificate leaf = H"e83bf2a9e888173fa604ba90fb4a0c638e81eaa1"
  /Applications/Aloud.app: valid on disk
  /Applications/Aloud.app: satisfies its Designated Requirement
  ```
  It is **not** the ad-hoc case. It is a stable, non-ad-hoc DR that survives rebuilds.
- **SETTLED 2026-08-10 — it works.** The spike below was run; `SMAppService.mainApp`
  accepted `/Applications/Aloud.app` signed by the self-signed "Aloud Dev" identity on
  macOS 26.6, with `TeamIdentifier=not set` and no Apple anchor:
  `NotFound → register: Ok → Enabled → unregister: Ok → NotRegistered`. No
  `BTMErrorDomain -98`, no `kSMErrorInvalidSignature`. The TN3127 reading below (BTM keys
  on a **stable, non-ad-hoc** designated requirement) is the correct one; the DTS
  "Apple-issued identity" reply was about an embedded helper + LaunchDaemon, not `mainApp`.
  Evidence and the two surprises it turned up: [`2026-08-10-launch-at-login.md`](2026-08-10-launch-at-login.md).
  The paragraph below is preserved as written on 2026-08-09.
- **UNVERIFIED (as of 2026-08-09; see SETTLED above)** — Whether BTM will
  *construct an identifier* for a bundle whose leaf certificate has no Apple anchor and
  `TeamIdentifier=not set` is undocumented, and no forum thread reports either success or
  failure for a **self-signed (not ad-hoc) `mainApp`**. The `failed to construct identifier`
  / `BTMErrorDomain -98` failure above is exactly the shape a rejection would take, and that
  app *was* in `/Applications`.
  **What would confirm it:** a throwaway bundle signed with the same "Aloud Dev" identity
  calling `SMAppService.mainApp.register()` then printing `.status`, watched with
  `sudo log stream --debug --info --predicate "sender in {'ServiceManagement','BackgroundTaskManagement','smd','backgroundtaskmanagementd'}"`.
  This was **not** run — registering a login item is persistent system configuration and
  needs the owner's explicit go-ahead. **This is the one thing M4 should spike before
  committing to a design.**

### Path, moving, and reinstalling

- **VERIFIED (Apple DTS, verbatim)** — registration **outlives the bundle**. Reporter:
  *"the entry in System Settings > General > Login Items is not gone. It's still there and
  enabled. It also persists across reboots & after removing all copies of my app."*
  Apple: *"That is the current behavior. The state is persisted to preserve user intent."*
  Recovery is `sfltool resetbtm` + reboot.
  https://developer.apple.com/forums/thread/707482
- **VERIFIED** — BTM does record a path: the failure log in thread 722338 shows
  `appURL=/Applications/Time Doctor 2.app` inside identifier construction.
- **VERIFIED** — Apple's macOS 26 user guide, on the Open at Login list: *"A warning triangle
  indicates moved or deleted items won't launch."*
  https://support.apple.com/en-my/guide/mac-help/mtusr003/26/mac/26
- **VERIFIED (Apple DTS, Quinn)** — *"Many macOS subsystems rely on the Launch Services
  database to track the location of apps. These can fail mysteriously if the LS database is
  having problems… the #1 tip for resolving these problem is to find and delete any build of
  your app other than the one you're actively developing."*
  https://developer.apple.com/forums/thread/726826
  ⚠ Direct hazard for Aloud's workflow: `target/Aloud.app` and `/Applications/Aloud.app`
  are two builds of the same bundle id on disk at once, permanently.
- **LIKELY** — the record is created from the app URL and attributed via Launch Services +
  the code signature; bundle id alone is not sufficient. Apple publishes no explicit
  statement naming the primary key.
- **Could not find** any Apple statement requiring the app to be in `/Applications`. The
  nearest real constraint is **app translocation** (a quarantined app run from `~/Downloads`
  executes from a randomized read-only mount) — which does not apply here: Aloud is in
  `/Applications` and carries no `com.apple.quarantine` attribute (measured: only
  `com.apple.provenance` and `com.apple.macl`).

### The two alternatives

**`SMLoginItemSetEnabled(_:_:)` — dead, and wrong shape anyway.**
- **VERIFIED** — deprecated in **macOS 13.0**. Apple's platform record is literally
  `{"name":"macOS","introducedAt":"10.6","deprecatedAt":"13.0","message":"Please use SMAppService instead"}`.
  https://developer.apple.com/documentation/servicemanagement/smloginitemsetenabled(_:_:)
- **VERIFIED** — it cannot register the main app at all: it enables a **helper bundle in
  `Contents/Library/LoginItems`**, identified by that helper's bundle id.

**A hand-written `~/Library/LaunchAgents/<id>.plist`.**
- **VERIFIED — no signing requirement whatsoever.** It is a file the user's own account
  writes into the user's own directory; `launchd` execs `ProgramArguments`. No Developer ID,
  no notarization, no BTM identifier construction.
- **VERIFIED — it does appear in Login Items & Extensions on macOS 13+**, and Apple states
  the attribution rules verbatim: *"If a legacy `LaunchAgent` or `LaunchDaemon` doesn't have
  the `AssociatedBundleIdentifiers` key in its property list, instead of the app name,
  System Settings displays the organization name in the app's signing certificate. If the
  system can't attribute a `LaunchAgent` or `LaunchDaemon` to an app and the executable
  isn't signed, System Settings displays the executable name."*
  https://developer.apple.com/documentation/servicemanagement/updating-helper-executables-from-earlier-versions-of-macos
- **VERIFIED — the attribution catch that hits Aloud specifically.** Same article: *"The
  Team Identifier of the `Program` or `ProgramArguments` executable in the legacy property
  list must match that of the app bundle for the `AssociatedBundleIdentifiers` key."*
  Aloud has `TeamIdentifier=not set` (measured above), so **`AssociatedBundleIdentifiers`
  attribution will not bind** and the Login Items entry will **LIKELY** read **"Aloud Dev"**
  (the certificate's organization name) rather than "Aloud".

### `tauri-plugin-autostart` — verified from source, and it is not what the README implies

- **VERIFIED** — current published version **2.5.1** (2025-10-27, crates.io API).
- **VERIFIED** — `plugins/autostart/Cargo.toml` pins **`auto-launch = "0.5"`**. Cargo `^0.5`
  does **not** match 0.6.
  `gh api repos/tauri-apps/plugins-workspace/contents/plugins/autostart/Cargo.toml --jq .content | base64 -d`
- **VERIFIED** — the plugin offers exactly two macOS modes and no third:
  ```rust
  pub enum MacosLauncher {
      #[default]
      LaunchAgent,
      AppleScript,
  }
  ```
  Its own crate doc line reads *"Supports Windows, Mac (via AppleScript or Launch Agent), and
  Linux."* Grepping the plugin source for `smapp` returns nothing.
  **No published version of the plugin uses `SMAppService`.**
- **VERIFIED (trap)** — the `.app`-path correction is applied **only in AppleScript mode**:
  ```rust
  let app_path = if parts.len() == 2
      && matches!(self.macos_launcher, MacosLauncher::AppleScript)
  { format!("{}.app", parts.first().unwrap()) } else { exe_path };
  ```
  So in the **default `LaunchAgent` mode** the plist points at the raw Unix executable
  `/Applications/Aloud.app/Contents/MacOS/aloud`, **not** the bundle — the plugin's own
  comment says this "results in seeing a Unix Executable in macOS login items", and it only
  fixes it for the other mode.
- **VERIFIED** — what `auto-launch` 0.5.0 writes: `~/Library/LaunchAgents/{app_name}.plist`
  containing only `Label` = `{app_name}`, `ProgramArguments` = `[app_path, ...args]`,
  `RunAtLoad` = `<true/>`. **No `AssociatedBundleIdentifiers`, no reverse-DNS label.** For
  Aloud that would be `~/Library/LaunchAgents/Aloud.plist` with `Label` `Aloud`. And
  `is_enabled()` is just `self.get_file().exists()` — it does **not** ask BTM, so it reports
  `true` even after the user has disabled the item in System Settings.
- **VERIFIED** — AppleScript mode shells
  `osascript -e 'tell application "System Events" to make login item …'`, which requires an
  Apple Events / Automation TCC consent against System Events — a **new permission prompt**
  Aloud does not currently need.
- **VERIFIED** — the SMAppService request is **open and unmerged**: plugins-workspace issue
  **[#2720](https://github.com/tauri-apps/plugins-workspace/issues/2720)**, "Use SMAppService
  to register the app to LoginItems instead of an AppleScript", opened 2025-05-26 by the
  author of `smappservice-rs`, still open (last "+1" 2025-12-15). Its motivating complaint is
  concrete: the plugin's path produces **two macOS notification pop-ups** with unclear
  attribution, versus one with the native API.
- **VERIFIED** — upstream `auto-launch` **0.6.0** (2026-01-10) *did* add
  `MacOSLaunchMode::SMAppService` via `smappservice-rs`, but the bump into the plugin was
  **closed unmerged** — PR #3307, opened and closed 2026-03-02 (14 minutes later); earlier
  renovate attempts #3208/#3209 also closed. The plugin is stuck on 0.5.
- **VERIFIED (implementation route)** — `objc2-service-management` **0.3.2** exists on
  crates.io with an `SMAppService` feature, and is the same 0.3.x objc2 generation Aloud
  already uses (`objc2-app-kit` 0.3.2, `objc2-foundation` 0.3). So `SMAppService` is
  reachable **directly from Rust** with no Swift helper and no new crate generation.
  Alternatives: `smappservice-rs` 0.1.3, or `auto-launch` 0.6 directly.

### What this means for M4

1. **Do not adopt `tauri-plugin-autostart` expecting `SMAppService`.** Verified from source:
   it pins `auto-launch = "0.5"`, exposes only `LaunchAgent` and `AppleScript`, and the 0.6
   bump was closed unmerged. Its default mode writes a LaunchAgent pointing at the bare
   `Contents/MacOS/aloud`, and its AppleScript mode would introduce a brand-new Automation
   TCC prompt.
2. **Spike `SMAppService.mainApp.register()` on the self-signed bundle before designing
   around it.** Apple documents no CA requirement and Aloud's DR is stable and non-ad-hoc —
   but BTM's behaviour for a non-Apple-anchored leaf is unverified, and its failure mode
   (`failed to construct identifier`) is undocumented. Owner sign-off first: this writes
   persistent system state that survives app deletion (`sfltool resetbtm` to undo).
3. **Never treat "registered" as "enabled".** `register()` returns `kSMErrorAlreadyRegistered`
   on a second call, and a user toggle-off yields `.requiresApproval`, not `.notRegistered`.
   Read `.status` on every launch as the single source of truth, route the user with
   `SMAppService.openSystemSettingsLoginItems()`, and never re-`register()` to "fix" a
   deliberate opt-out.
4. **If the fallback is a hand-written LaunchAgent, name it `com.andriileso.aloud.plist`,
   point `ProgramArguments` at the bundle (via `/usr/bin/open -a`), not the bare binary, and
   expect the Login Items row to read "Aloud Dev".** `TeamIdentifier=not set` means
   `AssociatedBundleIdentifiers` will not bind, and launching the inner executable directly
   bypasses LaunchServices — the very mechanism the `NSServices` selection path depends on
   (see Traps).

---

## 3. Global-shortcut rebinding UI

### Capturing a chord — and which paths need Accessibility

- 🚩 **VERIFIED — `NSEvent.addGlobalMonitorForEvents` requires the Accessibility grant.**
  Apple's Discussion, verbatim: *"Key-related events may only be monitored if accessibility
  is enabled or if your application is trusted for accessibility access (see
  `AXIsProcessTrusted`)."* And: *"Note that your handler will not be called for events that
  are sent to your own application."*
  https://developer.apple.com/documentation/appkit/nsevent/addglobalmonitorforevents(matching:handler:)
  **Never use this.** It is both accessibility-gated *and* useless for recording a chord —
  it explicitly excludes our own app's events. It would violate the standing "never
  Accessibility" constraint for zero benefit.
- **VERIFIED — `NSEvent.addLocalMonitorForEvents` does not mention accessibility at all.**
  *"Installs an event monitor that receives copies of events the system posts to this app
  prior to their dispatch."* No TCC gate: the events are already destined for us. This is
  the accessibility-free native capture path if one is ever needed.
  https://developer.apple.com/documentation/appkit/nsevent/addlocalmonitorforevents(matching:handler:)
- **VERIFIED — `performKeyEquivalent(with:)` / `keyDown(with:)` are plain responder
  overrides, no permission.**
  https://developer.apple.com/documentation/appkit/nsresponder/performkeyequivalent(with:)
- **VERIFIED — dispatch order, which is what makes webview capture work.** Apple, *Cocoa
  Event Handling Guide*: *"The global NSApplication object dispatches events it recognizes
  as potential key equivalents… It sends a `performKeyEquivalent:` message to the key
  NSWindow object. This object passes key equivalents down its view hierarchy… If no object
  in the view hierarchy handles the key equivalent, NSApp then sends `performKeyEquivalent:`
  to the menus."* **Views win before the main menu.**
  https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/HandlingKeyEvents/HandlingKeyEvents.html

### Capturing inside the Tauri WKWebView

- **VERIFIED — WebKit gives the page first crack at ⌘-chords, before the app menu.**
  `WebViewImpl::performKeyEquivalent`, WebKit `Source/WebKit/UIProcess/mac/WebViewImpl.mm`,
  verbatim comment: *"Pass key combos through WebCore if there is a key binding available
  for this event. This lets webpages have a crack at intercepting key-modified keypresses."*
  If the WKWebView is first responder it returns `YES`, and the menu item never fires.
- **VERIFIED — and if the page does *not* consume the event, WebKit re-sends it to the
  menu** (`WebViewImpl::resendKeyDownEvent` → `[NSApp sendEvent:event]`).
  **Consequence: `preventDefault()` in the page DOES suppress app-level menu key
  equivalents (Cmd+W, Cmd+Q, Cmd+H).** This is the opposite of the usual assumption and is
  exactly what a chord recorder needs.
- **VERIFIED — but `preventDefault()` does NOT stop true system shortcuts.** Cmd+Space,
  Cmd+Tab, Ctrl+arrows, Cmd+Shift+3/4/5 are consumed above the app, so no `keydown` fires
  and there is nothing to prevent. Apple's canonical list:
  https://support.apple.com/en-us/102650 (HT201236 redirects here) — *"Command–Space bar:
  Show or hide the Spotlight search field"*, *"Command-Tab: Switch to the next most recently
  used app"*, *"Control–Up Arrow: Open Mission Control"*, *"Shift-Command-5: … take a
  screenshot"*.
  **LIKELY** (mechanism) that these never reach the app process; confirm by logging every
  `keydown` in the settings page and pressing each.
- **VERIFIED — use `event.code`, not `event.key`.** MDN: *"The `KeyboardEvent.code`
  property represents a physical key on the keyboard (as opposed to the character generated
  by pressing the key)… this property returns a value that isn't altered by keyboard layout
  or the state of the modifier keys."*
  https://developer.mozilla.org/en-US/docs/Web/API/KeyboardEvent/code
- **VERIFIED — `event.code` maps 1:1 onto the crate's key names and onto the same macOS
  virtual keycodes.** MDN's "Code values on Mac" table gives `KeyR` → `0x0F`, `Digit5` →
  `0x17`, `ArrowUp` → `0x7E`, `Backquote` → `0x32`; `global-hotkey`'s table is identical
  (`Code::KeyR => Some(0x0f)`, `Code::Digit5 => Some(0x17)`, `Code::ArrowUp => Some(0x7e)`,
  `Code::Backquote => Some(0x32)`),
  `~/.cargo/registry/src/index.crates.io-.../global-hotkey-0.8.0/src/platform_impl/macos/mod.rs:411-520`.
  **The webview `code` string is directly feedable to the plugin's parser.**
- **VERIFIED — non-US layouts and dead keys are a *display* problem, not a capture problem.**
  `code` is layout-invariant; Carbon also registers by virtual keycode (physical position).
  MDN warns the same `code` is `'` on Dvorak and `A` on AZERTY, *"That makes it impossible to
  use the value of `code` to determine what the name of the key is to users."* For dead keys,
  `key` is the literal string `"Dead"`. **Store `code`; label with `key`.**
- **VERIFIED** — `metaKey` is ⌘ on Mac (*"On Macintosh keyboards, this is the ⌘ Command
  key."*). Map `metaKey` → `Super`; the plugin parses `CmdOrCtrl` → `Modifiers::SUPER` on macOS.
- **LIKELY** — `keyup` is unreliable while ⌘ is held on macOS (long-standing AppKit
  behaviour). Build the recorder on `keydown` only; never require a matching `keyup`.
- **VERIFIED (gap that will bite a European keyboard)** — `IntlBackslash` (the ISO section
  key, `kVK_ISO_Section 0x0A` in MDN's Mac table) is in **neither** the plugin's `parse_key`
  nor `key_to_scancode`. An ISO-keyboard user pressing it gets a hard failure.

### Registering: what the plugin does and does not tell you

All paths under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

- **VERIFIED — the macOS backend is Carbon `RegisterEventHotKey`. Not a `CGEventTap`, not an
  `NSEvent` global monitor. NO Accessibility grant needed.**
  `global-hotkey-0.8.0/src/platform_impl/macos/mod.rs:116-123`:
  ```rust
  let result = RegisterEventHotKey(
      scan_code, mods, hotkey_id,
      GetApplicationEventTarget(), 0, &mut hotkey_ref,
  );
  ```
  `inOptions` is `0`, i.e. **non-exclusive**. Events arrive via `InstallEventHandler` on
  `GetApplicationEventTarget()`. Chain: plugin `Cargo.toml` → `global-hotkey = "0.8"` →
  `global-hotkey-0.8.0`. **This confirms the current design is Accessibility-free and must
  stay that way.**
- 🚩 **VERIFIED — exactly one code path in this crate creates a `CGEventTap`: media keys.**
  `macos/mod.rs:140-147` routes `MediaPlayPause | MediaTrackNext | MediaTrackPrevious |
  MediaFastForward | MediaRewind` (`is_media_key`, :522-531) into `start_watching_media_keys()`,
  which calls `CGEventTapCreate(...)` at :206-213. Apple: *"Event taps receive key up and key
  down events if one of the following conditions is true: The current process is running as
  the root user. Access for assistive devices is enabled."*
  https://developer.apple.com/documentation/coregraphics/1454426-cgeventtapcreate
  The tap masks `SystemDefined` rather than key events, so the *documented* gate is arguably
  not triggered (**LIKELY**, not verified, that it works untrusted on macOS 26) — but the
  risk is real and the mitigation is free: **reject media keys in the rebind UI and the tap
  is never created.** Failure is at least reported (`Error::FailedToWatchMediaKeyEvent` when
  the tap comes back null), not silent.
- **VERIFIED — API surface** (`tauri-plugin-global-shortcut-2.3.2/src/lib.rs`):
  `register` (:131), `on_shortcut` (:143), `unregister` (:182), `unregister_all` (:220),
  `is_registered` (:232). Error type `Error::{GlobalHotkey(String), RecvError, Tauri}`
  (`src/error.rs:7-16`).
- **VERIFIED — failures are reported as `Err`, not swallowed.** `register_internal`
  (:89-102) runs the OS call **before** inserting into its map, so a failed register leaves
  no phantom state.
- **VERIFIED — but the error is lossy.** `impl From<global_hotkey::Error> for Error` flattens
  to `Self::GlobalHotkey(value.to_string())`, and upstream already discarded the `OSStatus`:
  `macos/mod.rs:125-130` returns `FailedToRegister(format!("RegisterEventHotKey failed for {}", hotkey.key))`.
  **The `-9878` code never surfaces.** You cannot programmatically distinguish
  "already registered in this process" from any other registration failure.
- **VERIFIED — `eventHotKeyExistsErr = -9878` means something much narrower than "taken".**
  `…/HIToolbox.framework/Headers/CarbonEventsCore.h:142-152`, verbatim: *"Returned from
  RegisterEventHotKey when an attempt is made to register a hotkey that is already registered
  in the current process. (Note that it is not an error to register the same hotkey in
  multiple processes.)"*
- **VERIFIED — Carbon hotkeys are non-exclusive by default.** `CarbonEvents.h:15428-15490`:
  *"Only one such combination can exist for the current application… The same hot key can,
  however, be registered by multiple applications."* `kEventHotKeyExclusive` exists but must
  be requested — and the plugin passes `0`.
- **VERIFIED — a chord held by ANOTHER app produces NO error.** `register()` returns `Ok(())`
  and the event is then silently shadowed. **There is no API here that reports cross-app
  contention.**
- **VERIFIED — `is_registered` is local bookkeeping only.** Its own doc comment (:229-231):
  *"If the shortcut is registered by another application, it will still return `false`."*
  Implementation is a `contains_key` on the plugin's own map. **Never present it as
  availability.**
- **VERIFIED — shortcut string syntax.** `Shortcut::from_str` → `global-hotkey-0.8.0/src/hotkey.rs:168-231`:
  split on `+`, modifiers first, exactly one main key. Modifier tokens (case-insensitive):
  `OPTION|ALT`, `CONTROL|CTRL`, `COMMAND|CMD|SUPER`, `SHIFT`,
  `COMMANDORCONTROL|COMMANDORCTRL|CMDORCTRL|CMDORCONTROL`. Unparseable →
  `HotKeyParseError::{UnsupportedKey, EmptyToken, InvalidFormat}` → `Error::GlobalHotkey(String)`.
  Modifiers-only (`"Shift+Ctrl"`) is an **error, not a panic** (regression test at
  `hotkey.rs:451-455`).
- **VERIFIED — `unregister` on a non-registered shortcut silently returns `Ok(())`**
  (`macos/mod.rs:163-165` — the `if let Some(...)` simply falls through).
- ⚠ **VERIFIED — the current Aloud wiring makes a bad chord fatal at launch.**
  `Builder::build()`'s setup does `manager.register(shortcut)?` (`lib.rs:403`), so anything
  passed to `with_shortcuts` that fails aborts plugin setup. `src/bin/aloud.rs:139-141`
  currently uses `.with_shortcuts([REGION_SHORTCUT]).expect(...)`. **A user-persisted chord
  routed through this path turns a bad save into a startup panic.**
- **VERIFIED — the JS side needs an explicit capability grant.** The plugin's
  `permissions/default.toml` reads *"No features are enabled by default, as we believe the
  shortcuts can be inherently dangerous"*, with `permissions = []`. Aloud has **no
  `capabilities/` directory today** — if the settings webview calls the plugin over IPC, one
  must be added.

### Detecting "that chord is taken"

- **VERIFIED — a failed `register()` is not sufficient, and is not even the common case.**
  Three distinct outcomes:
  1. **Parse failure** → `Err` before any OS call. Reportable precisely.
  2. **Already registered by *this* process** → `-9878` → `Err`, but the string does not say why.
  3. **Owned by macOS or another app** → `noErr`, `register()` returns **`Ok(())`**, and the
     event is silently shadowed. **The API cannot detect this case.**
- Therefore the only reliable UX is a **static denylist** of the documented system chords
  (§ Apple 102650) checked at capture time, plus a **post-save liveness confirmation**: ask
  the user to press the new chord and confirm a `ShortcutState::Pressed` arrived.
- **VERIFIED (Apple HIG)** — *"Avoid using the Control key as a modifier. The system uses
  Control in many systemwide features and shortcuts."*
  https://developer.apple.com/design/human-interface-guidelines/keyboards

### Where to persist the choice

- **VERIFIED — on macOS all three Tauri app-dir APIs resolve to the same place.**
  `tauri-2.11.5/src/path/desktop.rs:238-260` joins `dirs::{config_dir, data_dir,
  data_local_dir}` with the identifier, and `dirs-6.0.0/src/mac.rs:10-13` makes all of those
  `$HOME/Library/Application Support`. **For Aloud: `~/Library/Application Support/com.andriileso.aloud`.**
  (`app_log_dir` → `~/Library/Logs/<id>`, which is where `aloud.log` already lives.)
- ⚠ **VERIFIED — the identifier comes from `tauri.conf.json`, NOT from `Info.plist`.** The
  source is `self.0.config().identifier`, baked in at compile time by
  `tauri::generate_context!()`. It never reads `CFBundleIdentifier` at runtime. Today
  `tauri.conf.json` says `com.andriileso.aloud` and `packaging/make-app.sh` writes the same
  string into the bundle plist — **two independent hard-coded copies of the same identifier.**
  If they ever drift, config silently lands in a different directory while everything else
  keeps working.
- **VERIFIED — no sandbox concern.** `dirs-sys` resolves home via `$HOME` first; Aloud is not
  sandboxed, so this is the real home. (If a sandbox were ever added, the path silently moves.)
- **VERIFIED — these functions only *compute* a path.** `create_dir_all` before the first write.
- **VERIFIED — `tauri-plugin-store` is not required.** `Manager::path()` is in tauri core, so
  `app.path().app_config_dir()? + serde_json + std::fs` is complete. Adding the store plugin
  would only add an IPC surface and another capability grant.

### What this means for M4

1. **Record the chord with a `keydown` listener in the settings WKWebView and persist
   `event.code` plus the four modifier booleans.** Never `NSEvent.addGlobalMonitorForEvents` —
   Apple documents it as accessibility-gated *and* it excludes our own app's events, so it
   would breach the no-Accessibility constraint for nothing.
2. **Hard-block the five media keys in the rebind UI.** They are the only path in this whole
   dependency chain that calls `CGEventTapCreate` and can therefore pull in an
   Accessibility / Input-Monitoring prompt. Blocking them costs one `match` arm.
3. **Move the user chord off `Builder::with_shortcuts` and register it at runtime with
   explicit error handling.** The builder path propagates a register failure into plugin
   setup and kills launch; the current `.expect(...)` makes a bad persisted chord a startup
   panic. Unregister the old chord before registering the new one, and roll back on `Err`.
4. **Never claim availability from `register()` or `is_registered()`.** Ship a static
   denylist of the documented system chords checked at capture time, then a "press it now to
   confirm" step after saving — `register()` returns `Ok` for chords owned by macOS or by
   another app, and `is_registered()` only reports our own bookkeeping.

---

## 4. Packaging and distribution

### What macOS requires of a hand-assembled bundle

- **VERIFIED — the only required subdirectory is `Contents/MacOS/`.** Apple's Bundle
  Programming Guide, Table 2-5, marks `MacOS` *"(Required)"*; `Resources`, `Frameworks`,
  `PlugIns` carry no such marker. It also sanctions the current layout: *"you may put other
  standalone executables (such as command-line tools) in this directory as well."*
  https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html
- **VERIFIED — "expected" keys** (Table 2-6): `CFBundleName`, `CFBundleDisplayName`,
  `CFBundleIdentifier`, `CFBundleVersion`, `CFBundlePackageType`, `CFBundleSignature`,
  `CFBundleExecutable`. `make-app.sh` writes all of these **except `CFBundleSignature`** — a
  legacy four-character creator code, also absent from shipping apps like Google Chrome.
  Harmless.
- **VERIFIED — `LSMinimumSystemVersion` does not gate launch.** Apple: *"The **App Store**
  uses this key to indicate the macOS releases on which your app can run."* No launch-time
  role is documented; its absence is harmless for direct distribution.
  https://developer.apple.com/documentation/bundleresources/information-property-list/lsminimumsystemversion
- **VERIFIED — `CFBundleSupportedPlatforms` / `DTPlatformVersion` / `BuildMachineOSBuild`
  are not required.** `CFBundleSupportedPlatforms` has no page in Apple's Information
  Property List reference at all, and — decisively — `/Applications/Google Chrome.app`, a
  shipping notarized Developer ID app, has **none of the three**. A notarized production app
  lacking them proves notarization does not require them.
- **VERIFIED — `Contents/_CodeSignature/CodeResources` is created by `codesign`** when
  signing a bundle. It is the resource seal (`rules`/`rules2` + `files`/`files2` hashes).
  Aloud's already seals `Resources/icon.icns` by hash and `MacOS/aloud-ocr` by cdhash +
  requirement; `codesign -dvvv` reports `Sealed Resources version=2 rules=13 files=2`.

### Nested helper signing — and a live defect in the current script

- **VERIFIED — `codesign` does not sign nested code automatically; without `--deep` the
  outer signing operation *fails*.** `man codesign`, OPERATION: *"Code nested within bundle
  directories must already be signed or the signing operation will fail, unless the `--deep`
  option is given, in which case any unsigned nested code will be recursively signed before
  proceeding, using the same signing options and parameters."* This is exactly why
  `make-app.sh` reaches for `--deep`.
- **VERIFIED — `--deep` is deprecated in the man page itself:** *"(DEPRECATED for signing as
  of macOS 13.0)"*, followed by *"All signing options will be applied, in turn, to all nested
  content. This is almost never what you want."*
- **VERIFIED — Apple's docs have a section literally titled "Avoid deep code signing":**
  *"Don't pass the `--deep` option to `codesign` when you sign code."* Prescribed order:
  *"Sign code from the inside out. That is, if component `A` depends on component `B`, sign
  `B` before you sign `A`."*
  https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac
- **VERIFIED (Apple DTS, Quinn — "`--deep` Considered Harmful")** — *"It applies the same
  code signing options to every code item that it signs, something that's not appropriate in
  general… The first issue is fundamental to how `--deep` works, and is the main reason you
  should not use it. Indeed, on macOS it may cause the trusted execution system to block your
  program from running."*
  https://developer.apple.com/forums/thread/129980
- **VERIFIED — `Contents/MacOS/` is a correct, Apple-sanctioned home for the helper.**
  Apple's placement table lists "help app, helper tool → macOS → `Contents/MacOS/`" (and
  `Contents/Helpers/`). `Contents/Resources/` would be **wrong**: *"If you put content in the
  wrong location, you may encounter hard-to-debug code signing and distribution problems…
  incorrectly placed code might work during day-to-day development, but might cause problems
  during notarization."* The existing deliberate choice is right.
  https://developer.apple.com/documentation/bundleresources/placing-content-in-a-bundle
- ⚠ **VERIFIED (live defect, measured)** — the helper's code-signing identifier is
  `aloud-ocr`, derived from the *filename* (`man codesign`: *"each path derives its identifier
  independently from its Info.plist or pathname"*), giving DR
  `identifier "aloud-ocr" and certificate leaf = H"e83b…"`. Apple prescribes `-i <BundleID>`
  for non-bundled code (e.g. `com.example.flying-animals.pig-jato`). A bare `aloud-ocr`
  identifier is squattable by any other developer's binary of the same filename.

### The four signing tiers

Apple's own `syspolicy_check` (macOS 14+), run locally against the real bundle:

```
$ syspolicy_check distribution /Applications/Aloud.app
Notary Ticket Missing … Severity: Fatal
$ syspolicy_check notary-submission /Applications/Aloud.app
Codesign Error  File: Aloud.app/Contents/MacOS/aloud-ocr  Severity: Fatal
$ spctl -a -vvv /Applications/Aloud.app
/Applications/Aloud.app: rejected      origin=Aloud Dev
```

| | (a) ad-hoc `-` | (b) self-signed | (c) Dev ID unnotarized | (d) Dev ID notarized + stapled |
|---|---|---|---|---|
| Launch locally (no quarantine) | works | works | works | works |
| Launch after download (quarantined) | blocked → Open Anyway | blocked → Open Anyway | blocked → Open Anyway | clean |
| **TCC grant survives rebuild** | **no** | **yes** | yes | yes |
| TCC survives version bump | no | yes *(LIKELY)* | yes *(LIKELY)* | yes *(LIKELY)* |
| Gatekeeper / `spctl` | reject | reject | reject | accept |
| Notarization possible | no | **no** | n/a | n/a |

- **VERIFIED — ad-hoc breaking TCC is documented by Apple, not just community lore.**
  TN3127: *"Unsigned code has no DR. Ad hoc signed code, called Sign to Run Locally by Xcode,
  has a DR but it's tied to that specific version of the code. In both cases macOS can't
  reliably track the identity of the code… If you tweak the code and run it again, macOS
  repeats that prompt."* **The M3 finding is confirmed by primary source.**
  https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements
- **VERIFIED — a self-signed app can never be notarized.** *"If you use any other
  certificate — like a Mac App Distribution certificate, or a **self-signed certificate** —
  notarization fails with the following message: `The binary is not signed with a valid
  Developer ID certificate.`"*
  https://developer.apple.com/documentation/security/resolving-common-notarization-issues
- **LIKELY** — the version-bump row follows deductively from the DR mechanism (a
  cert-leaf-pinned DR contains no version string) but Apple never enumerates it per-tier.
  Confirming it needs a fresh VM, a grant, then a rebuild at a bumped version.

**This confirms the M3 conclusion and sharpens it: development and distribution are
genuinely different concerns, and the self-signed cert is the correct answer for the
development one and a dead end for the distribution one.**

### Gatekeeper and quarantine on macOS 26

- **VERIFIED — right-click → Open is gone from Apple's documentation.** Both current support
  pages document only: System Settings → Privacy & Security → scroll down → **"Open Anyway"**
  → *"The warning prompt reappears and, if you're absolutely sure that you want to open the
  app anyway, you can click Open."* Neither page mentions Control-click. The Open Anyway
  button *"is available for about an hour after you try to open the app."*
  https://support.apple.com/en-us/102445 ·
  https://support.apple.com/guide/mac-help/open-an-app-by-overriding-security-settings-mh40617/mac
  ⚠ **This contradicts `README.md`**, which currently instructs: *"The first launch,
  right-click the app in Finder and choose **Open**"*. That guidance is stale for macOS
  15/26 and must be rewritten.
- **VERIFIED — "damaged" and "unidentified developer" are distinct alerts.** Apple's exact
  distinctions: malicious content / revoked authorization → *"will damage your computer"*;
  **modified or damaged** → *"can't be opened"*; known malware → can't be opened **and moved
  to Trash**. Unsigned-but-intact gets *"Apple cannot check 'X' for malicious software"* with
  Move to Trash / Done.
- **VERIFIED — a fourth, non-Gatekeeper failure is easy to misdiagnose:** `The application
  "X" can't be opened.` — a missing `x` bit, arch mismatch, unauthorized restricted
  entitlements, or "some other code signing problem."
  https://developer.apple.com/forums/thread/706442 (Quinn/DTS)
- **VERIFIED — Apple declines to document the quarantine xattr as API:** *"The
  `com.apple.quarantine` extended attribute is **not documented as API**. If you need to add,
  check, or remove quarantine from a file programmatically, use the `quarantinePropertiesKey`
  property."* So `xattr -d com.apple.quarantine` is a supported debugging step, not a
  supported interface.
- **VERIFIED (measured)** — the installed app carries no `com.apple.quarantine` (only
  `com.apple.provenance` and `com.apple.macl`). **It was never downloaded, so the Gatekeeper
  path has never actually been exercised on this machine** — `spctl` rejects it, but nothing
  enforces that without quarantine.

### TCC persistence across updates

- **VERIFIED — TCC keys on the designated requirement.** TN3127, verbatim: *"macOS solves
  this problem by recording your app's **DR** in its database of apps authorized to access
  the microphone. Each time your app tries to access the microphone, macOS checks that this
  version of the app satisfies the original DR. In short, the DR is all about code identity."*
  Not the path, not the hash. **The M3 claim is confirmed.**
- Consequences: rebuild + same cert → survives (**VERIFIED**, and empirically confirmed in
  M3); version bump → survives (**LIKELY** — `CFBundleShortVersionString` appears nowhere in
  the DR).
- **VERIFIED — install method is mostly irrelevant because the DR governs, with one real
  trap: Gatekeeper app translocation.** *"when the person first opens your app, Gatekeeper
  randomizes its path as returned from `bundleURL`… Gatekeeper only performs this
  translocation on first launch"* and *"The person moves your app to a different location,
  then launches it. Gatekeeper doesn't translocate."* Running straight from a mounted DMG or
  an unzipped download runs from a randomized read-only path.
  https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution
- **VERIFIED — unzipping over the top diverges by tool:** *"Unix-y unarchiving tools, like
  `tar` and `unzip`, don't propagate quarantine"* while *"User-level unarchiving tools
  preserve quarantine."*

### The macOS 15 screen-recording re-authorization — narrower than reported, and Aloud is outside it

- **VERIFIED — it is API-triggered, not universal.** macOS 15 release notes, ScreenCaptureKit
  → Deprecations: *"Applications utilizing deprecated APIs for content capture such as
  `CGDisplayStream` & `CGWindowListCreateImage` can trigger system alerts indicating they
  might be able to collect detailed information about the user. Developers need to migrate to
  `ScreenCaptureKit` and `SCContentSharingPicker`."* (120910350)
  https://developer.apple.com/documentation/macos-release-notes/macos-15-release-notes
- **VERIFIED — macOS 15.1 softened it:** *"Applications using our deprecated content capture
  technologies now have enhanced user awareness policies. Users will see fewer dialogs if
  they regularly use apps in which they have already acknowledged and accepted the risks."*
  (133431080)
  https://developer.apple.com/documentation/macos-release-notes/macos-15_1-release-notes
- **VERIFIED — macOS 26 release notes contain no ScreenCaptureKit or Gatekeeper entries.**
  No further change on record.
- ⚠ **Correction against a plausible-sounding conclusion:** Aloud does **not** call
  `CGDisplayStream` or `CGWindowListCreateImage`. It shells out to the system
  `screencapture -i -x` tool (`src/capture/macos.rs:56`) and only touches
  `CGPreflightScreenCaptureAccess` *after* a failed capture. **The documented trigger for the
  repeat-authorization alerts therefore does not apply to Aloud's code, and "migrate to
  ScreenCaptureKit" is not an M4 action item.** Do not rewrite a working capture path on the
  strength of a release note about APIs this project does not use.
- **VERIFIED — the escape hatch is gated and unavailable.**
  `com.apple.developer.persistent-content-capture` is documented as *"whether a **Virtual
  Network Computing (VNC)** app needs persistent access to screen capture"* and *"Before your
  app can use this entitlement, request permission to use it by submitting the […] form."*

### Hardened runtime and entitlements

- **VERIFIED — hardened runtime is mandatory for notarization.** *"To upload a macOS app to
  be notarized, you must enable the Hardened Runtime capability."* Failure string: *"The
  executable does not have the hardened runtime enabled."*
  https://developer.apple.com/documentation/security/hardened-runtime
- 🚩 **VERIFIED — `NSScreenCaptureUsageDescription` DOES NOT EXIST. Do not add it.** Two
  independent confirmations: (1) Apple's Information Property List reference contains **zero**
  keys matching "creen"; (2) decisively, `strings` on the live
  `/System/Library/PrivateFrameworks/TCC.framework/Support/tccd` on macOS 26.6 (25G72)
  enumerates all 40 `*UsageDescription` keys TCC recognises — `NSCameraUsageDescription`,
  `NSMicrophoneUsageDescription`, `NSAppleEventsUsageDescription`, … — and **no
  screen-capture key is among them**, while `kTCCServiceScreenCapture` *is* present as a
  service name. **Screen Recording is a TCC service with no purpose-string key at all.**
  Adding an invented key would be a pure silent no-op — exactly the `NSRequiredContext` class
  of bug, in reverse.
- **VERIFIED (measured)** — the app has neither hardened runtime nor a secure timestamp:
  `codesign -dvvv` shows `flags=0x0(none)` and `Signed Time=` rather than `Timestamp=`
  (Apple: *"the presence of `Signed Time` in the output indicates the binary doesn't have a
  secure timestamp"*). Both are blockers only for notarization, not for local use.
- **VERIFIED — hardened-runtime exceptions a Rust app might need**, exact identifiers:
  `com.apple.security.cs.allow-jit`, `…allow-unsigned-executable-memory`,
  `…allow-dyld-environment-variables`, `…disable-library-validation`,
  `…disable-executable-page-protection`, `…debugger`. Plain Rust needs **none** — *"The
  Hardened Runtime doesn't affect the operation of most apps."* **LIKELY** that a
  WKWebView-based Tauri frontend also needs none, because the JIT runs in a *system* process,
  not ours; confirming means enabling `-o runtime` and exercising the app.
- **VERIFIED** — manual-signing convention: *"Don't include an entitlement if the value is
  false"*, and never ship `com.apple.security.get-task-allow`.

### Distribution formats

Apple's own comparison
(https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution):

- **zip** — *"You can't sign a zip archive, so any files or folders you include that aren't
  covered by your code signature may be tampered with by an attacker."* Build with
  `ditto -c -k --keepParent`. Cannot be stapled directly.
- **dmg** — *"You can sign a disk image, which protects all files and folders you include
  from modification after you sign it."* Must be UDIF read-only zip-compressed (`UDZO`),
  signed with a **code**-signing identity. Supports stapling.
- **pkg** — *"An installer package is the best choice if your product contains multiple
  components, must be copied to specific locations, or if you need to run custom code during
  installation."* Requires a **Developer ID Installer** identity — a *different* certificate
  from Developer ID Application.
- *"If you distribute your product using nested containers, only notarize the outermost
  container."* And *"If you don't staple the ticket to your distribution file, Gatekeeper
  might block a user from installing or using your product while their Mac is offline."*
- **For retaining a TCC grant the format is not the deciding factor — the DR is.** The
  format-level differences that actually matter are quarantine propagation (zip diverges by
  unarchiver) and translocation on first launch.

### What this means for M4

1. **Drop `--deep` and sign inside-out, giving the helper a scoped identifier:**
   `codesign --force --sign "Aloud Dev" -i com.andriileso.aloud.aloud-ocr Contents/MacOS/aloud-ocr`
   first, then the outer bundle. Apple deprecated `--deep` for signing in macOS 13 and warns
   it "may cause the trusted execution system to block your program from running"; the
   filename-derived `aloud-ocr` identifier is squattable.
2. **Keep the self-signed certificate and never let the build silently fall back to
   `--sign -`.** Apple documents ad-hoc DRs as tied to that exact build, which is precisely
   the Screen Recording regrant loop M3 hit. Make a missing "Aloud Dev" cert **fail the
   build**, not warn — the current warn-and-continue reproduces the original bug.
3. **Do not add `NSScreenCaptureUsageDescription`, hardened runtime, entitlements, or
   `--timestamp`.** The first key does not exist on macOS 26.6; the rest buy nothing under a
   self-signed cert (notarization rejects it outright) and only add ways to break a working
   local build. Gate any notarization work behind `syspolicy_check distribution` running clean.
4. **Fix `README.md`'s first-launch instructions.** Right-click → Open is no longer in
   Apple's documentation for macOS 15/26; the documented path is System Settings → Privacy &
   Security → **Open Anyway**, available for about an hour after the blocked attempt.

---

## 5. Other things macOS silently requires

Hunting specifically for the `NSRequiredContext` *class* of bug: a step or key whose absence
produces no error, no log, and no visible symptom other than "it doesn't work".

- **VERIFIED — Cmd+C / Cmd+V in the settings window depend on Tauri's default app menu, and
  it is one method call away from being deleted.** macOS routes ⌘-chords through
  `NSApp.mainMenu`'s `performKeyEquivalent:`; an app with no main menu has no clipboard
  shortcuts even though the right-click context menu still works. This was
  [tauri#1055](https://github.com/tauri-apps/tauri/issues/1055) (closed 2021-02-25) and is a
  recurring Apple-forums complaint for menu-bar apps
  (https://developer.apple.com/forums/thread/713987 — open since Sep 2022, no Apple answer).
  Tauri 2.11.5 mitigates it by default: `Builder` sets `enable_macos_default_menu: true`
  (`app.rs:1620`) and `build()` installs `Menu::default()` when no menu was set
  (`app.rs:2244-2249`), which includes an **Edit** submenu with undo/redo/cut/copy/paste/
  select-all (`menu/menu.rs:214-227`). Aloud passes `.menu(&menu)` to `TrayIconBuilder`, not
  to `Builder`, so the default app menu survives. **Calling `Builder::menu(...)` or
  `enable_macos_default_menu(false)` in M4 would silently remove copy/paste from the settings
  window.**
  **LIKELY** (not verified) that these key equivalents fire for an *accessory*-policy app,
  whose menu bar is never displayed — the mechanism is `NSApplication.sendEvent:` dispatch,
  which does not require the bar to be visible. **Smoke-test it manually in M4** rather than
  assuming.
- 🚩 **VERIFIED — launch-at-login will make Aloud steal focus at every login, and Tauri
  gives you no way to turn it off.** `tao` sets `activate_ignoring_other_apps: true` by
  default (`tao-0.35.3/src/platform_impl/macos/app_delegate.rs:107`) and calls
  `ns_app.activateIgnoringOtherApps(ignore)` unconditionally in
  `applicationDidFinishLaunching` (`app_state.rs:291-293`). `tao` exposes
  `set_activate_ignoring_other_apps` (`src/platform/macos.rs:336`), but **grepping
  `tauri-2.11.5`, `tauri-runtime-wry-2.11.4` and `tauri-runtime-2.11.3` for
  `activate_ignoring_other_apps` returns nothing** — it is not wired through. That is
  [tauri#15017](https://github.com/tauri-apps/tauri/issues/15017), open since 2026-03-01, a
  contributor greenlit 2026-04-10, not landed. Mitigation on macOS 14+ is partial: the
  argument is ignored and it degrades to a plain `activate()` *request* the system may
  decline. **Test what login actually looks like before shipping the feature.**
- ⚠ **VERIFIED mechanism, UNVERIFIED consequence — the default autostart mode launches the
  inner binary, not the bundle.** `tauri-plugin-autostart`'s LaunchAgent mode writes
  `ProgramArguments = ["/Applications/Aloud.app/Contents/MacOS/aloud"]`, bypassing
  LaunchServices. Aloud's `NSServices` selection path is documented (README, "Building") as
  registering only from an installed `.app` launched as a bundle. **Whether the Service still
  registers when launchd execs the inner binary directly is untested and is the single
  highest-risk unknown in the autostart feature.** Confirm by writing the plist by hand,
  logging out and back in, and checking `~/Library/Logs/Aloud/aloud.log` plus
  `Services → Read Aloud`.
- **VERIFIED — `SMAppService` registration outlives the app.** Apple DTS: *"That is the
  current behavior. The state is persisted to preserve user intent."* Deleting or
  reinstalling Aloud does **not** clear the Login Items row; only `sfltool resetbtm` + reboot
  does. A dev-loop `rm -rf /Applications/Aloud.app` leaves a stale enabled entry behind.
  https://developer.apple.com/forums/thread/707482
- **VERIFIED — `auto-launch` 0.5's `is_enabled()` is a file-existence check**, not a BTM
  query. It returns `true` after the user has switched the item off in System Settings. Any
  "Launch at login" toggle wired to it will show the wrong state with no error.
- **VERIFIED — `global-hotkey`'s `register()` returns `Ok(())` for a chord already owned by
  macOS or by another app** (non-exclusive Carbon registration, `inOptions = 0`), and
  `unregister()` on an unregistered chord also returns `Ok(())`. Both are silent successes
  that mean "nothing happened".
- **VERIFIED — `tao::Window::set_focus()` early-returns on a non-visible window.** No error,
  no log. `show()` must come first.
- **VERIFIED — `NSScreenCaptureUsageDescription` does not exist.** Adding it produces no
  error and no behaviour — a silent no-op that would look like a fix.
- ⚠ **VERIFIED — the bundle identifier is hard-coded twice.** `tauri.conf.json` (`identifier`)
  drives `app_config_dir()`; `packaging/make-app.sh` (`CFBundleIdentifier`) drives TCC, the
  DR and `NSServices`. Nothing checks they agree. Drift would silently relocate user config
  while everything else kept working.
- **VERIFIED — the plugin's IPC permissions default to empty.**
  `permissions/default.toml`: *"No features are enabled by default, as we believe the
  shortcuts can be inherently dangerous"*. Aloud has no `capabilities/` directory, so a
  settings page calling `register`/`unregister` over IPC will be denied.
- **VERIFIED (Apple DTS, Quinn)** — two builds of the same bundle id on disk confuse
  LaunchServices: *"the #1 tip for resolving these problem is to find and delete any build of
  your app other than the one you're actively developing."* Aloud permanently has
  `target/Aloud.app` **and** `/Applications/Aloud.app`.
  https://developer.apple.com/forums/thread/726826
- **VERIFIED — modifying `Info.plist` after `codesign` invalidates the seal.** `make-app.sh`
  currently writes the plist *before* signing, which is correct. Any M4 step that edits the
  bundle plist after signing (e.g. stamping a version) breaks the DR check and re-opens the
  M3 TCC bug — the app would still launch, so the failure would be silent.

### What this means for M4

1. **Do not touch `Builder::menu()` or `enable_macos_default_menu`.** Tauri's default app
   menu is the only reason ⌘C/⌘V will work in the settings window, and removing it produces
   no error.
2. **Treat launch-at-login as a spike, not a task.** Three separate unknowns stack on it:
   whether `SMAppService` accepts a self-signed bundle, whether the `NSServices` path
   survives a launchd-exec of the inner binary, and whether tao's unconditional
   `activateIgnoringOtherApps` produces a visible focus grab at login. Verify each on the
   real machine before writing implementation tasks.
3. **Add an identifier-consistency assertion to `packaging/make-app.sh`** — read `identifier`
   out of `tauri.conf.json` and fail the build if it differs from the `CFBundleIdentifier`
   being written. Same shape as the existing `NSServices` post-merge check, which already
   caught this class of bug once.
4. **Keep every bundle mutation before the `codesign` call, and make the missing-certificate
   case a hard failure.** The current fallback to ad-hoc with a warning silently reintroduces
   the exact TCC bug that cost M3 a debugging session.

---

## Traps — silent-failure modes, in priority order

| # | Trap | Symptom | Evidence |
|---|---|---|---|
| 1 | `register()` returns `Ok(())` for a chord owned by macOS or another app | Rebound shortcut simply never fires; UI says "saved" | `global-hotkey` passes `inOptions = 0`; `CarbonEvents.h`: *"it is not an error to register the same hotkey in multiple processes"* |
| 2 | `SMAppService.mainApp.register()` on a self-signed bundle — **unproven** | `BTMErrorDomain -98` / `failed to construct identifier`, or a login item that never launches | Apple documents no CA requirement; DTS on record says "Apple-issued identity"; no thread covers self-signed `mainApp` |
| 3 | Autostart LaunchAgent execs `Contents/MacOS/aloud`, bypassing LaunchServices | App starts at login but **Services → Read Aloud** may vanish | Plugin applies the `.app`-path fix only in AppleScript mode; README documents the Service needs an installed bundle |
| 4 | `tao::set_focus()` is a no-op on a non-visible window | Settings window opens behind everything, unfocused | `tao-0.35.3/.../window.rs:677-685` |
| 5 | Losing Tauri's default app menu kills ⌘C/⌘V in the settings window | Right-click paste works, keyboard paste doesn't | tauri#1055; `tauri-2.11.5/src/menu/menu.rs:214-227`, `app.rs:1620` |
| 6 | Ad-hoc signing fallback re-keys the DR to the cdhash | Screen Recording silently dead after every rebuild, while System Settings still shows it enabled | TN3127; already cost M3 a session; `make-app.sh` still *warns* rather than failing |
| 7 | tao calls `activateIgnoringOtherApps` unconditionally at launch | App grabs focus at every login once autostart is on | `tao-0.35.3/.../app_delegate.rs:107` + `app_state.rs:291-293`; tauri#15017 not landed |
| 8 | Media keys are the one path that creates a `CGEventTap` | An Accessibility/Input-Monitoring prompt appears in an app that promises never to ask | `global-hotkey-0.8.0/.../macos/mod.rs:140-147, 206-213` |
| 9 | `auto-launch`'s `is_enabled()` is a file-existence check | Toggle shows "on" after the user turned it off in System Settings | `auto-launch` 0.5.0 `src/macos.rs` |
| 10 | `SMAppService` registration persists after the app is deleted | Stale enabled Login Items row; re-register fails confusingly | Apple DTS thread 707482; `sfltool resetbtm` |
| 11 | `unregister()` on an unregistered chord returns `Ok(())` | Cleanup appears to succeed while the old chord stays live | `global-hotkey-0.8.0/.../macos/mod.rs:163-165` |
| 12 | `is_registered()` never reports other apps | "Available" shown for a chord that is taken | Plugin doc comment, `lib.rs:229-231` |
| 13 | Bundle identifier hard-coded in two files | User config silently written to a different directory | `tauri.conf.json` vs `packaging/make-app.sh`; nothing cross-checks |
| 14 | `NSScreenCaptureUsageDescription` does not exist | Adding it looks like a fix and does nothing | `strings` on `tccd`, macOS 26.6 — 40 usage-description keys, none for screen capture |
| 15 | Editing `Info.plist` after `codesign` | App still launches; TCC grant silently invalid | `_CodeSignature/CodeResources` seals the plist |
| 16 | Plugin IPC permissions default to empty | Settings page's `register` call is denied with no obvious cause | `permissions/default.toml`; Aloud has no `capabilities/` |
| 17 | Two builds of the same bundle id on disk | Intermittent LaunchServices misbehaviour | Apple DTS thread 726826; `target/Aloud.app` + `/Applications/Aloud.app` |
| 18 | `IntlBackslash` is in neither the parser nor the scancode table | ISO-keyboard users get a hard failure on that key | `global-hotkey-0.8.0/src/hotkey.rs` + `macos/mod.rs:411-520` |

---

## Contradictions with existing Aloud docs

1. **`README.md` §Building — "right-click the app in Finder and choose Open"** is stale.
   Apple's current documentation for macOS 15/26 describes only System Settings → Privacy &
   Security → **Open Anyway**; Control-click no longer appears in either support page
   (102445, mh40617). Rewrite in the same change set as any M4 packaging work.
2. **`README.md` §Building — "The app is ad-hoc signed"** contradicts `M3-carry-forward.md`
   and the shipped `make-app.sh`, which sign with the self-signed "Aloud Dev" identity.
   Measured on the installed bundle: `Authority=Aloud Dev`. The README paragraph describes
   the pre-fix state.
3. **`CLAUDE.md` Stack — "Windows packaging (M6) is undecided"** stands; nothing here changes
   it. Noted only so the M4 packaging work is not read as settling M6.
