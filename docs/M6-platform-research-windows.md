# M6 — Windows platform-integration research

Research pass for the Windows port (M6), done **before** planning, to close the gap named in
[`M3-carry-forward.md`](M3-carry-forward.md): M3 was planned by verifying Rust *crate* APIs
exhaustively and doing zero research into what the *operating system* requires. First real use hit
four bugs that 80 tests missed; three were documented platform requirements.

**Nothing here was tested.** There is no Windows machine on this host and WinRT bindings do not
compile on macOS. Every claim carries a confidence tag:

- `VERIFIED` — read in a primary source, cited.
- `LIKELY` — strong secondary evidence, named.
- `UNVERIFIED` — say so plainly; what would confirm it is stated (usually "test on the PC").

Empirical questions are collected at the end under **"Must be verified on the PC"** and are written
to be pasted verbatim into a `BKM/PC-Queue/` brief for a machine with no context.

Researched 2026-08-09. Facts about Tauri/tao/tray-icon were read from the `dev` branch of the
upstream repos via `gh api`, not from release docs.

---

## 1. Tray app with a settings window

### 1.1 The notification-area overflow — a new tray icon is hidden by default

- **VERIFIED — a newly added tray icon goes to the overflow flyout, not the visible tray.**
  "When an icon is added to the notification area on Windows 7, it is added to the overflow section
  of the notification area by default. This area contains notification area icons that are active,
  but not visible in the notification area."
  ([Notifications and the Notification Area](https://learn.microsoft.com/en-us/windows/win32/shell/notification-area))
- **VERIFIED — there is no programmatic promotion.** Same page: "Only the user can promote an icon
  from the overflow to the notification area, although in certain circumstances the system can
  temporarily promote an icon into the notification area as a short preview (under one minute)."
- **VERIFIED — the temporary preview is the *only* automatic visibility, and admins can switch it
  off.** The GPO "Turn off automatic promotion of notification icons to the taskbar" exists on both
  Windows 10 and 11: "If you enable this policy setting, newly added notification icons aren't
  temporarily promoted to the Taskbar."
  ([Taskbar policy settings](https://learn.microsoft.com/en-us/windows/configuration/taskbar/policy-settings))
- **VERIFIED — there is no supported object model to read or drive the overflow either.** Raymond
  Chen: "In the case of the taskbar, there is no supported object model for notification icons. UI
  Automation is the best you can do," and hidden icons have no UI for UI Automation to see.
  ([The Old New Thing, 2025-09-29](https://devblogs.microsoft.com/oldnewthing/20250929-00/?p=111637))

**This is the single biggest UX difference from macOS.** On macOS the menubar item is always
visible. On Windows, Aloud's first launch will look — to a user who does not know about the
overflow chevron — like the app failed to start. There is no API fix; only first-run guidance.

### 1.2 Tray icon identity, and what survives a rebuild

- **VERIFIED — `tray-icon` (which Tauri 2 wraps) identifies the icon by `hWnd` + `uID`, never by
  `guidItem`, and never calls `NIM_SETVERSION`.** The `NOTIFYICONDATAW` it fills sets
  `uFlags`, `hWnd`, `uID: tray_id`, `uCallbackMessage`, `hIcon`, `szTip`, `dwState` — no
  `NIF_GUID`, no `NOTIFYICON_VERSION_4`. `uID` comes from a process-lifetime `COUNTER.next()`.
  (`gh api repos/tauri-apps/tray-icon/contents/src/platform_impl/windows/mod.rs`, `register_tray_icon`
  and `TrayIcon::new`.)
- **VERIFIED — Microsoft calls the GUID method preferred, and `NOTIFYICON_VERSION_4` strongly
  recommended.** "A registered GUID that identifies the icon. This value overrides **uID** and is
  the recommended method of identifying the icon."
  ([NOTIFYICONDATAW](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw));
  "Unless there is a compelling reason to do otherwise, it is strongly recommended that you use the
  NOTIFYICON_VERSION_4 version" ([Notification Area](https://learn.microsoft.com/en-us/windows/win32/shell/notification-area)).
- **VERIFIED — and this is the closest Windows analogue to the macOS signing/TCC bug.** If you *do*
  use `guidItem`, "The path of the binary file is included in the registration of the icon's GUID
  and cannot be changed. Settings associated with the icon are preserved through an upgrade only if
  the file path and GUID are unchanged… The only exception to a moved file occurs when both the
  original and moved binary files are Authenticode-signed by the same company."
  ([NOTIFYICONDATAW, Troubleshooting](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw))
  So a **stable Authenticode signing identity buys persistence of the user's tray settings across a
  binary move** — the same *shape* of problem as macOS TCC binding to the designated requirement.
- **UNVERIFIED — whether the user's "promote out of overflow" choice survives an Aloud restart,
  a rebuild, and a reinstall** under the `hWnd`+`uID` scheme tray-icon actually uses. Must be tested
  on the PC (question 1 below). If it does **not** survive, that is the Windows equivalent of the
  macOS "grant died on every rebuild" bug and will waste a debugging session if not anticipated.
- **VERIFIED — an uninstalled app's icon lingers in the Notification Area Icons control panel for up
  to seven days**, and changes made there have no effect
  ([Notification Area](https://learn.microsoft.com/en-us/windows/win32/shell/notification-area)).
  Expect confusing state while iterating.

### 1.3 `icons/icon.ico` — the exact spec, and why M6 is blocked on it

- **VERIFIED — a missing `icons/icon.ico` is a hard build failure, not a silent one.** `tauri-build`
  resolves the icon as `windows_attributes.window_icon_path` → first `bundle.icon` entry ending in
  `.ico` → `"icons/icon.ico"`, and then:
  `return Err(anyhow!("`{}` not found; required for generating a Windows Resource file during tauri-build"))`.
  It is embedded as resource id `"32512"` (`IDI_APPLICATION`).
  (`gh api repos/tauri-apps/tauri/contents/crates/tauri-build/src/lib.rs`, the `target_triple.contains("windows")` block.)
  Aloud's `tauri.conf.json` currently declares `"icon": ["icons/icon.png"]` and `icons/` holds only
  `icon.png` + `tray.png` — so the first Windows `cargo build` fails immediately with that message.
- **VERIFIED — the size set Microsoft specifies for an application icon.** "Application icons and
  Control Panel items: The full set includes 16x16, 32x32, 48x48, and 256x256 (code scales between
  32 and 256). The .ico file format is required. For Classic Mode, the full set is 16x16, 24x24,
  32x32, 48x48 and 64x64."
  ([Icons — Design basics](https://learn.microsoft.com/en-us/windows/win32/uxguide/vis-icons))
- **VERIFIED — the high-DPI ladder for a *small* (tray-sized) icon** is 16 @ 96 dpi (100%),
  20 @ 120 dpi (125%), 24 @ 144 dpi (150%), 32 @ 192 dpi (200%). For a *large* icon: 32 / 40 / 48 /
  64. (Same page, "For high dpi".)
- **VERIFIED — colour depth.** "ICO design for 32-bit (alpha included) + 8-bit + 4-bit… Only a
  32-bit copy of the 256x256 pixel image should be included, and only the 256x256 pixel image should
  be compressed to keep the file size down." (Same page.) The 8-bit/4-bit versions exist for remote
  desktop's default colour setting.
- **Caveat, stated in the source:** that page carries a banner — "This design guide was created for
  Windows 7 and has not been updated for newer versions of Windows." The modern
  [app icon design guide](https://learn.microsoft.com/en-us/windows/apps/design/iconography/app-icon-design)
  covers metaphor, grid, colour and shadow but **contains no `.ico` size table at all** (checked).
  The Windows-7-era table is still the only official size guidance for a Win32 `.ico`.

**Concrete spec to produce:** one `icons/icon.ico` containing **16, 20, 24, 32, 40, 48, 64, 256**
at 32-bit BGRA with alpha; 256 PNG-compressed inside the container; optionally 8-bit and 4-bit
copies of 16/32/48 for RDP. 16/20/24 must be redrawn, not downscaled — see 1.4.

### 1.4 Tray icon rendering — Tauri does NOT use the `.ico`, and does not use `LoadIconMetric`

- **VERIFIED — Tauri's tray icon comes from RGBA pixels, at whatever size you hand it.**
  `TrayIconBuilder::icon(Image)` → `impl TryFrom<Image<'_>> for tray_icon::Icon` →
  `tray_icon::Icon::from_rgba(rgba, width, height)`
  (`crates/tauri/src/image/mod.rs:150-156`), and on Windows `from_rgba` calls `CreateIcon(..., width,
  height, ...)` — the source image's exact pixel dimensions
  (`tray-icon/src/platform_impl/windows/icon.rs`).
- **VERIFIED — Microsoft's guidance is the opposite of that.** "If only a 16x16 pixel icon is
  provided, it is scaled to a larger size in a system set to a high dpi value. This can lead to an
  unattractive result. It is recommended that you provide both a 16x16 pixel icon and a 32x32 icon
  in your resource file. Use `LoadIconMetric` to ensure that the correct icon is loaded and scaled
  appropriately."
  ([NOTIFYICONDATAW, `hIcon`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw))
  `tray-icon` has a `from_path` route that uses `LoadImageW` with `LR_DEFAULTSIZE`, but **Tauri's
  public tray API does not expose it** — only the RGBA path.
- **Consequence:** the embedded `icon.ico` governs the taskbar/Explorer/Alt-Tab icon, and has
  **nothing to do with the tray glyph**. The tray glyph is whatever PNG/RGBA Aloud passes at runtime,
  scaled by the shell. Handing it the existing macOS-sized `tray.png` will produce a blurry tray icon
  at 100% and 150% scaling.
- **Carry-forward is already relevant here:** `HANDOFF.md` records that at 22 pt on macOS only the
  square and the "A" read; the crosshair and speaker corners do not. On Windows the *base* size is
  16 px, smaller than macOS's 22 pt — so the simplification decision is forced, not optional.

### 1.5 Showing and focusing the settings window from a tray click

- **VERIFIED — the documented foreground rules.** A process may call `SetForegroundWindow` only if
  it is a desktop app, the foreground process has not called `LockSetForegroundWindow`, and no menus
  are active — **and** at least one of: the foreground lock time-out has expired; *the calling
  process is the foreground process*; the calling process was started by the foreground process;
  there is no foreground window; *the calling process received the last input event*; or either
  process is being debugged. "It is possible for a process to be denied the right to set the
  foreground window even if it meets these conditions." When denied, "Windows flashes the taskbar
  button of the window to notify the user."
  ([SetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow))
- **VERIFIED — a tray-menu click satisfies those rules, because `tray-icon` already forces the
  foreground for the menu.** Microsoft documents the requirement: "To display a context menu for a
  notification icon, the current window must be the foreground window before the application calls
  `TrackPopupMenu`… However, when the current window is the foreground window, the second time this
  menu is displayed, it appears and then immediately disappears. To correct this, you must force a
  task switch… by posting a benign message"
  ([TrackPopupMenu, Remarks](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-trackpopupmenu)).
  `tray-icon` implements exactly this: `SetForegroundWindow(hwnd); TrackPopupMenu(...); PostMessageW(hwnd, WM_NULL, 0, 0);`
  (`tray-icon/src/platform_impl/windows/mod.rs:545-558`). So by the time a menu item fires, Aloud's
  process *is* the foreground process and the "calling process is the foreground process" condition
  holds.
- **VERIFIED — the actual trap is ordering, not permission.** tao's `Window::set_focus` is a no-op
  unless the window is already visible and not minimised:
  `if is_visible && !is_minimized && !is_foreground { force_window_active(hwnd) }`
  (`tao/src/platform_impl/windows/window.rs:139-150`). **Calling `set_focus()` on a hidden window
  does nothing at all, silently.** The correct pattern is `show()` → `unminimize()` if needed →
  `set_focus()`.
- **VERIFIED — and tao has a foreground-stealing fallback worth knowing about.**
  `force_window_active` first tries `SetForegroundWindow`; if that returns false it **synthesises a
  left-Alt key down/up via `SendInput`** to acquire foreground rights
  (`tao/src/platform_impl/windows/window.rs:1504-1529`). Two consequences: a stray synthetic Alt is
  injected into whatever app currently has focus, and this is the kind of behaviour AV heuristics
  notice. Not a reason to avoid Tauri — a reason to know where an unexplained Alt keystroke came
  from.

### 1.6 WebView2 — is Tauri a runtime system dependency on Windows?

**This is the highest-value question in this document, and the answer is: yes, conditionally, and
the constraint is satisfiable but not free.**

- **VERIFIED — Microsoft does NOT claim WebView2 ships with Windows 10.** Its own words: "The
  Evergreen WebView2 Runtime will be included as part of the Windows 11 operating system… The vast
  majority of Windows 10 devices have the WebView2 Runtime installed already… A small number of
  Windows 10 devices don't have the WebView2 Runtime installed. We recommend that you handle this
  edge case."
  ([Distribute your app and the WebView2 Runtime](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution))
  It also says an app "should check whether the WebView2 Runtime is present… and install the Runtime
  if it is missing."
- **VERIFIED, and a correction to a source you will otherwise trust:** Tauri's own docs state "On
  Windows 10 (April 2018 release or later) and Windows 11, the WebView2 runtime is distributed as
  part of the operating system"
  ([Tauri — Windows Installer](https://v2.tauri.app/distribute/windows-installer/)). **That is
  stronger than Microsoft's own statement** and conflicts with the "small number of Windows 10
  devices" caveat above. Treat Microsoft as authoritative. For Aloud's *own* target PC this is moot
  (it is a modern Windows install), but it matters the moment Aloud is distributed.
- **VERIFIED — detection is cheap and documented.** Read the `pv (REG_SZ)` value under
  `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`
  (per-machine) or the `HKCU\Software\...` equivalent (per-user); absent, empty, or `0.0.0.0` means
  not installed. Or call `GetAvailableCoreWebView2BrowserVersionString` and check for `nullptr`.
  (Same distribution page.)
- **VERIFIED — the four Tauri deployment modes and their real costs**
  ([Tauri — Windows Installer](https://v2.tauri.app/distribute/windows-installer/), cross-checked
  against the [Tauri config reference](https://v2.tauri.app/reference/config/)):

  | `webviewInstallMode` | Installer size delta | Needs internet at install | Notes |
  |---|---|---|---|
  | `downloadBootstrapper` (default) | ~0 | **Yes** | Downloads the ~2 MB bootstrapper, which then downloads the runtime |
  | `embedBootstrapper` | ~+1.8 MB | **Yes** | Bootstrapper is embedded; the runtime is still fetched |
  | `offlineInstaller` | ~+127 MB | No | Embeds the full Evergreen standalone installer |
  | `fixedVersion` | ~+180 MB | No | Ships a pinned runtime; no auto-update, no shared copy |
  | `skip` | 0 | — | App is non-functional if the runtime is absent |

  Microsoft states the Fixed Version binaries are "over 250 MB" on disk; Tauri's ~180 MB is the
  compressed installer delta. Both numbers are real, at different points in the pipeline.
- **VERIFIED — a Fixed Version silent trap on Windows 10 specifically.** "On Windows 10 devices,
  starting with Fixed Version 120, developers of unpackaged Win32 applications using Fixed Version
  are required to run [`icacls {path} /grant *S-1-15-2-2:(OI)(CI)(RX)` and `*S-1-15-2-1`] for Fixed
  Version to continue to work." (App Container change for the renderer process.) Windows 11 and
  packaged apps are unaffected. Also: "Fixed Version cannot be run from a network location or UNC
  path." (Distribution page.) An unpackaged Win32 app on Windows 10 is *exactly* Aloud's shape.
- **VERIFIED — the WebView2 *loader* is not a shipped DLL on the MSVC toolchain.** `tauri-build`
  copies `WebView2Loader.dll` next to the binary **only** for `CARGO_CFG_TARGET_ENV == "gnu"`; on
  `msvc` it instead runs `static_vcruntime::build()`
  (`crates/tauri-build/src/lib.rs`, the `match target_env` block). Microsoft confirms the loader can
  be statically linked (`WebView2Loader.lib`). **Build with the MSVC toolchain** and there is no
  loader DLL to ship.
- **LIKELY — WebView2 is only actually needed when a webview window is created.** `wry` calls
  `CreateCoreWebView2EnvironmentWithOptions` inside its webview build path
  (`gh api repos/tauri-apps/wry/contents/src/webview2/mod.rs:349`), not at `EventLoop` creation. Aloud's
  `tauri.conf.json` declares `"windows": []` today, so a *tray-only* build plausibly starts on a
  machine with no WebView2 and fails only when the settings window opens. **This is not a plan to
  rely on** — M4 adds a settings window, which makes WebView2 mandatory in practice. Must be tested
  on the PC (question 3).
- **UNVERIFIED — whether a WebView2 failure surfaces as a Rust `Err` or as a silent dead window.**
  The one upstream report found ([tauri#12030](https://github.com/tauri-apps/tauri/issues/12030))
  describes the window appearing "and then immediately closed" after the user uninstalled the
  runtime — i.e. a *silent* failure with no error dialog. Must be tested on the PC (question 3).

### What this means for M6

1. **Produce `icons/icon.ico` first — it is a hard build blocker, and the error message is explicit.**
   Sizes 16/20/24/32/40/48/64/256, 32-bit with alpha, 256 PNG-compressed. Redraw 16/20/24 by hand;
   do not downscale the 256.
2. **Treat the tray glyph as a separate asset from `icon.ico`.** Tauri feeds raw RGBA to `CreateIcon`
   at source dimensions and never touches the `.ico` or `LoadIconMetric`. Supply a 16-px-designed
   tray asset (and ideally swap it per DPI at runtime); do not reuse the macOS 22-pt `tray.png`.
3. **Plan the first-run overflow message into the product, not into a bug report.** Windows will hide
   Aloud's tray icon on first launch and there is no API to promote it. First run must say, in words:
   *"Aloud is running. Its icon is under the ⌃ chevron in the taskbar — drag it out to keep it
   visible."* Without this the app reads as broken.
4. **Never call `set_focus()` on a hidden window.** `show()` first; `set_focus()` on a hidden window
   silently does nothing. Wire the settings window open as show → unminimize → set_focus, in that
   order, from the tray menu handler (where foreground rights are already held).
5. **Decide `webviewInstallMode` deliberately, as an owner call** (see Constraint conflicts).
   For Andrii's own PC, `downloadBootstrapper` is fine. For anything distributable, `skip` is not an
   option and `offlineInstaller`/`fixedVersion` are the only zero-dependency-honest choices.
6. **Build with the MSVC toolchain, not GNU** — GNU drops a `WebView2Loader.dll` beside the exe
   (a shipped runtime file) and skips the static VC runtime step.
7. **Consider adopting `guidItem` + `NOTIFYICON_VERSION_4`** only if PC testing shows tray settings
   do not persist across restarts. It would require patching or bypassing `tray-icon`; do not do it
   speculatively.

---

## 2. Launch at login

### 2.1 The four mechanisms

**(a) `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`**

- **VERIFIED — the documented mechanism.** "Use `Run` or `RunOnce` registry keys to make a program
  run when a user logs on… These keys can be set for the user or the machine." Value data is "a
  command line no longer than 260 characters"; the order among multiple entries "is indeterminate."
  ([Run and RunOnce registry keys](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys))
- **LIKELY — no elevation needed for HKCU.** Microsoft never states it outright; HKLM is the
  per-machine variant that does. Confirmed indirectly: `auto-launch` 0.5.0 writes only HKCU and never
  requests elevation.
- **VERIFIED — survives a Tauri update, by design.** Tauri's NSIS uninstaller deletes the Run value
  **only when not updating**:
  ```nsi
  ; We do this when not updating (to preserve the registry value on updates)
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}
  ```
  (`crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`)
- **VERIFIED — but the uninstaller and the plugin can disagree on the value name.** The uninstaller
  keys on `${PRODUCTNAME}`; `tauri-plugin-autostart` writes under `app.package_info().name` **unless**
  `Builder::app_name()` overrides it. Set a custom `app_name` and uninstall leaves an orphan Run value
  pointing at a deleted exe.
- **VERIFIED — the documented start delay, and its limits.** "The system does not provide guarantees
  about how promptly the programs in the `Run` key are run. To improve the user experience, the system
  may choose to delay the execution of programs in the `Run` key and in the Startup group to a time
  when they are less likely to interfere with the foreground user experience or with each other."
  That is the entirety of Microsoft's commitment — no number, no ordering.
- **UNVERIFIED — the widely-quoted ~10 s figure and
  `HKCU\…\Explorer\Serialize\StartupDelayInMSec`.** No Microsoft documentation exists for that value;
  every hit was a tech blog or forum. **Treat it as folklore; do not code against it.** Confirmable
  only by measuring on the PC.

**(b) Startup folder shortcut**

- Paths: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup` (per-user),
  `%ProgramData%\…\StartUp` (all users). **VERIFIED** via
  [MITRE ATT&CK T1547.001](https://attack.mitre.org/techniques/T1547/001/).
- Per-user folder is user-writable → no elevation (**LIKELY**, filesystem ACLs). Same documented
  delay as the Run key (the Run/RunOnce text names "the Startup group" explicitly) — **VERIFIED**.
- Nothing in Tauri's NSIS template creates or removes a Startup `.lnk`; it would be entirely
  app-managed. `tauri-plugin-autostart` does not use it on Windows.

**(c) Task Scheduler task**

- **VERIFIED — registering without elevation is explicitly supported.** "From a low privilege
  process, you cannot register a task with the `RunLevel` property equal to
  `TASK_RUNLEVEL_HIGHEST`, but you can register a task with the `RunLevel` property equal to
  `TASK_RUNLEVEL_LUA`. The task actions will be run with low privileges."
  ([Security contexts for running tasks](https://learn.microsoft.com/en-us/windows/win32/taskschd/security-contexts-for-running-tasks))
- **VERIFIED — this is the documented way to auto-start an *elevated* app** (`RunLevel HIGHEST`),
  which is exactly why it is also the standard UAC-bypass-at-logon technique. Registering such a task
  itself requires an already-elevated process.
- **UNVERIFIED — whether it appears in Task Manager → Startup apps.** No Microsoft statement either
  way. This is precisely why the mechanism is attractive to malware and unattractive to a
  well-behaved app: it takes the toggle away from the user. Must be tested.
- **LIKELY — disabled state is readable** via `IRegisteredTask` / `ITaskDefinition.Settings.Enabled`,
  and the task survives app update/uninstall because it lives in `%WINDIR%\System32\Tasks`
  independently.

**(d) MSIX `uap5:StartupTask`**

- **VERIFIED — the only mechanism with a first-class, documented enable/disable/detect API.**
  ([uap5:StartupTask](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap5-startuptask),
  [StartupTask class](https://learn.microsoft.com/en-us/uwp/api/windows.applicationmodel.startuptask))
  - "Once enabled, the user is in control and can change the enabled state of your app at any time
    via the **Startup** page in **Settings** or the **Startup** tab in Task Manager."
  - `RequestEnableAsync()`: "If the task was disabled by the user using Task Manager, this method will
    not override their choice and the user must re-enable the task manually."
  - `StartupTask.State` returns `Enabled` / `Disabled` / **`DisabledByUser`** / `DisabledByPolicy` —
    a documented, structured answer to "did the user turn me off?"
  - Packaged desktop apps can set `Enabled="true"` in the manifest and skip the consent dialog.
  - Minimum: Windows 10 1709 (build 16299) for the `uap5` element.
- **VERIFIED — blocked for this project.** Tauri v2's bundler has no MSIX target:
  `BundleType = Deb | Rpm | AppImage | Msi | Nsis | App | Dmg`
  (`crates/tauri-utils/src/config.rs:132`). Shipping (d) means packaging outside Tauri's bundler.

### 2.2 `StartupApproved` — the disable-detection story

- **VERIFIED (negative) — Microsoft does not document this key.** A `site:learn.microsoft.com` search
  returns only Q&A threads and archived forum posts, no reference page. Every description of the
  binary format is community-derived.
- **LIKELY — the subkeys**: `HKCU\…\Explorer\StartupApproved\Run`, `…\StartupApproved\StartupFolder`,
  and HKLM equivalents including `…\Run32`.
- **LIKELY — the blob format**: 12 bytes. First DWORD `0x02` (also `0x06`) = enabled; `0x03` =
  disabled, with bytes 4–11 holding a `FILETIME` of when it was disabled. Enabled is literally
  `02 00 00 00 00 00 00 00 00 00 00 00`.
- **Corroborated by code that ships to every Tauri user** — `auto-launch` hard-codes exactly that
  constant:
  ```rust
  const TASK_MANAGER_OVERRIDE_ENABLED_VALUE: [u8; 12] = [0x02, 0,0,0, 0,0,0,0, 0,0,0,0];
  fn last_eight_bytes_all_zeros(bytes: &[u8]) -> Option<bool> { … }
  ```
  Note it does **not** read the first byte — it tests "are the last 8 bytes all zero", i.e. "is there
  no disable timestamp". Functionally equivalent for `0x02` vs `0x03`, but it would mis-report a
  hypothetical `0x06`-with-timestamp.
- **Answer to "can the app detect it was disabled?"** Yes for Run-key and Startup-folder mechanisms,
  by reading `StartupApproved` — but only via an undocumented format Microsoft is free to change. For
  MSIX it is a documented API. For Task Scheduler you read your own task's `Enabled`.

### 2.3 `tauri-plugin-autostart` — read from source, not the README

Read from `tauri-apps/plugins-workspace` `plugins/autostart/` and `zzzgydi/auto-launch` at the pinned
revision.

- **VERIFIED — version pinning matters and is easy to get wrong.** Plugin `2.5.1` declares
  `auto-launch = "0.5"`. crates.io lists `0.6.0, 0.5.0, 0.4.0, …` — **there is no 0.5.1**, so `"0.5"`
  resolves to `0.5.0` (commit `fedaeba7`, 2023-09-10). Everything on `auto-launch`'s default branch
  (the `windows-registry` crate, HKLM `WindowsEnableMode::Dynamic`, richer error handling) is **0.6.0
  and is NOT what you get**. Reading the repo's default branch to answer "what does the plugin do"
  gives the wrong answer.
- **VERIFIED — mechanism is the HKCU Run key, not Task Scheduler:**
  ```rust
  static AL_REGKEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
  hkcu.open_subkey_with_flags(AL_REGKEY, KEY_SET_VALUE)?
      .set_value(&self.app_name, &format!("{} {}", &self.app_path, &self.args.join(" ")))?;
  ```
  HKCU only in 0.5.0 → no elevation, and no HKLM fallback.
- **VERIFIED — the written command line is UNQUOTED**, and a trailing space is always appended when
  `args` is empty. Tauri's NSIS `perMachine` install dir is `$PROGRAMFILES64\${PRODUCTNAME}` =
  `C:\Program Files\Aloud\…` — **a path with a space**. A `currentUser` install lands in
  `%LOCALAPPDATA%\Aloud`, which is space-free only as long as the Windows account name is. This is a
  latent bug, not a theoretical one. Must be tested (question 7).
- **VERIFIED — it does read `StartupApproved`.** `enable()` writes the 12-byte enabled blob **only if
  the key already exists**, and silently skips otherwise.
- **VERIFIED — `is_enabled()` = Run value present AND StartupApproved not-disabled:**
  `Ok(al_enabled && task_manager_enabled.unwrap_or(true))`.
  **So a user disabling Aloud in Task Manager IS detectable** — `isEnabled()` flips to `false`. That
  is the good news for the settings toggle: it can tell the truth rather than lie.
- **VERIFIED — `disable()` does NOT clean up `StartupApproved`**, and in 0.5.0 it **errors if the Run
  value doesn't exist** (`delete_value` is not error-swallowed, unlike 0.6.0). Calling `disable()`
  twice, or after the user removed the entry by hand, likely returns an error. Must be tested
  (question 10).
- **VERIFIED — the macOS path is a completely different mechanism** (`~/Library/LaunchAgents/…plist`
  with `RunAtLoad`, or an AppleScript login item). The two platforms share an API and nothing else; a
  Windows regression cannot be reasoned about from macOS behaviour.
- **VERIFIED that the issue exists; its claim is UNVERIFIED —** `plugins-workspace` **#771**
  "[autostart] autostart on Windows is removed after one boot", open since 2023-11-28, **zero
  comments, no diagnosis**. Reporter says the HKCU Run value is created correctly, the app launches
  once, then the value disappears. Unreproduced by maintainers. Worth one deliberate two-reboot test
  (question 12) rather than discovering it in daily use.

### 2.4 SmartScreen / AV vs an unsigned self-registering binary

- **VERIFIED — "no reputation" is itself the trigger.** SmartScreen "provides reputation checks for
  apps, checking downloaded programs **and the digital signature used to sign a file**. If a URL, a
  file, an app, or a certificate **has an established reputation, users don't see any warnings. If
  there's no reputation, the item is marked as a higher risk and presents a warning**."
  ([Microsoft Defender SmartScreen](https://learn.microsoft.com/en-us/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/))
- **Is a Run-key write itself an AV heuristic trigger? Conditional, not flat.** **VERIFIED** that it
  is a catalogued adversary technique — MITRE ATT&CK **T1547.001**, whose detection analytic AN1032 is
  explicitly "Registry key creation/modification events under known Run/Startup keys **with new or
  unusual binary paths**… registry modification followed by process execution from non-standard
  directories." Read carefully: the Run-key write **alone** is not the signal; the signal is
  unsigned/unusual binary path **+** Run key **+** odd process lineage. An unsigned Tauri app
  installed to `%LOCALAPPDATA%\Aloud` writing a Run key hits two of three. **LIKELY** that this
  raises heuristic score with third-party AV; **UNVERIFIED** for Defender specifically.
  **Do not repeat "AV flags Run keys" as a flat fact.**

### What this means for M6

1. **Use `tauri-plugin-autostart` (HKCU Run key) and keep the default `app_name`.** Overriding
   `Builder::app_name()` desynchronises the plugin from Tauri's uninstaller and orphans the Run value.
2. **Make the settings toggle read `isEnabled()`, not a stored boolean.** The plugin already folds
   `StartupApproved` into that call, so the toggle can reflect a Task-Manager disable instead of
   lying. This was an explicit research question and the answer is favourable — use it.
3. **Guard `disable()`.** 0.5.0 errors when the Run value is already absent. Wrap it, or check
   `is_enabled()` first.
4. **Assume the install path may contain a space and verify the Run value is usable** (question 7).
   If it is broken, the fix is a wrapper that writes the quoted value directly — not a plugin bump,
   since `"0.5"` cannot reach the fixed 0.6.0 without an explicit dependency override.
5. **Do not build a startup-delay workaround.** The delay is documented as deliberate and unbounded;
   the `StartupDelayInMSec` number is folklore. Instead, make the tray registration resilient — §6.4
   shows `tray-icon` already waits for `TaskbarCreated`, and that is the correct posture.
6. **Do not switch to Task Scheduler.** It buys nothing here, may hide the toggle from the user, and
   is the elevated-autostart pattern — which is only relevant if Aloud were elevated, which §3 argues
   against.

---

## 3. Global-shortcut rebinding UI

### 3.1 `RegisterHotKey` semantics

All from
[RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey)
and [WM_HOTKEY](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-hotkey) — **VERIFIED**:

- **Which thread receives it:** "A handle to the window that will receive `WM_HOTKEY` messages… If
  this parameter is **NULL**, `WM_HOTKEY` messages are posted to the message queue of the **calling
  thread**." And: "The message is placed at the top of the message queue associated with the thread
  that registered the hot key." Also — "This function fails if you try to associate a hot key with a
  window created by another thread."
- **Collision behaviour — it FAILS, it does not silently steal.** "Typically, **RegisterHotKey** also
  fails if the keystrokes specified for the hot key have already been registered for another hot key.
  However, some pre-existing, **default hotkeys registered by the OS** (such as PrintScreen, which
  launches the Snipping tool) **may be overridden** by another hot key registration when one of the
  app's windows is in the foreground." Third-party collision → hard failure. OS-default collision →
  you may win it. That exception is the only documented "silent steal".
- **The error code:** `ERROR_HOTKEY_ALREADY_REGISTERED` = **1409 (0x581)**, "Hot key is already
  registered." (`ERROR_HOTKEY_NOT_REGISTERED` = 1419.)
  ([System error codes 1300–1699](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--1300-1699-))
  Detection is: return 0 → `GetLastError()` → 1409. **There is no API to ask *who* took it.**
- **Other documented gotchas:** `F12` is permanently reserved for the debugger. App hotkey IDs must be
  `0x0000`–`0xBFFF`. Re-registering the same `hWnd`+`id` **adds** a second hotkey rather than
  replacing it — you must `UnregisterHotKey` yourself. `MOD_NOREPEAT` (0x4000) suppresses auto-repeat.

### 3.2 Reserved combinations

- **VERIFIED — the `MOD_WIN` caveat, verbatim:** "**MOD_WIN** 0x0008 — Either WINDOWS key must be
  held down… **Keyboard shortcuts that involve the WINDOWS key are reserved for use by the operating
  system.**" Microsoft publishes no machine-readable list of which `Win+<key>` combos are taken; it
  publishes the [user-facing shortcut list](https://support.microsoft.com/en-us/windows/keyboard-shortcuts-in-windows-dcc61a57-8ff0-cffe-9796-cb9706c75eec)
  instead — ~60 entries including `Win+L`, `Win+R`, `Win+E`, `Win+D`, `Win+X`, `Win+V`, `Win+Shift+S`,
  `Win+Tab`, `Win+G`, `Win+J`, `Win+C`, arrows and digits. **Practical rule: any `Win+<letter>` is a
  coin flip and Microsoft adds new ones every release** (`Win+J` = Recall, `Win+C` = Copilot are
  recent). Do not use `MOD_WIN` for Aloud's default chord.
- **VERIFIED-by-design — `Ctrl+Alt+Del`** is the Secure Attention Sequence, handled by Winlogon below
  the window manager. No user-mode API — `RegisterHotKey` or `WH_KEYBOARD_LL` — can see or claim it.
  No single Learn page states "RegisterHotKey rejects it", so treat "cannot bind" as certain and "what
  error you get" as untested.
- **LIKELY unavailable:** `Win+L`, `Alt+Tab`, `Alt+Esc`, `Ctrl+Esc` (shell-owned).
  **VERIFIED overridable:** `PrintScreen`, named in the docs as an OS default that can be overridden.

### 3.3 UIPI and elevated windows — a correction to the project's own doc

`M3-carry-forward.md` landmine 3 and the design spec's risk table both state: *"Global hotkeys
silently fail against elevated windows on Windows. Unfixable OS behaviour. Document it; do not chase
it."* **That is correct for hook-based capture. It is NOT established for `RegisterHotKey`, which is
what Aloud actually uses.** Setting out both sides, because the honest answer is "test it", and
shipping documentation about a limitation nobody has observed is its own bug.

**What UIPI is documented to block — VERIFIED:**
- "UIPI prevents application processes running with lower privileges from using Windows messages to
  send information to a higher privilege process."
  ([UIPI](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-7/dd638394(v=ws.10)))
- The blocked list is: window-handle validation, `SendMessage`/`PostMessage` to higher-IL windows,
  **`SetWindowsHookEx` into a higher-privilege process**, journal hooks, DLL injection — "a standard
  user process can't log the keystrokes the user types into an administrative application."

**Evidence FOR the claim — but only for the hook path:**
- **VERIFIED** — Microsoft states the symptom in product terms for PowerToys: "These are the two
  scenarios where PowerToys will not work: **Intercepting certain types of keyboard strokes**;
  Resizing / moving windows", and lists **"PowerToys Run — Use shortcut"** among the affected
  features. ([PowerToys and administrator mode](https://learn.microsoft.com/en-us/windows/powertoys/administrator))
- **LIKELY** — AutoHotkey's FAQ: "Hotkeys are also blocked, so for instance, a non-elevated program
  cannot spy on input intended for an elevated program." But AHK implements most hotkeys via the
  **keyboard hook**, so this sentence is about the hook path, not `RegisterHotKey`.

**Evidence AGAINST the claim for `RegisterHotKey` specifically:**
- **LIKELY** — a well-sourced Windows-internals compendium states: "UIPI didn't prevent processes from
  triggering key combinations registered with `RegisterHotKey()` though, which is what explorer uses
  for things like Alt+Tab", with a concrete exploit chain. Tagged LIKELY only: its own footnote is a
  Hacker News comment.
- **Mechanism argument from Microsoft's own text:** `WM_HOTKEY` is posted by the **system** ("the
  system posts the `WM_HOTKEY` message to the message queue of the window with which the hot key is
  associated"), not by a lower-integrity process sending to a higher one. UIPI's rule is about
  *cross-process sends*. `RegisterHotKey`'s Remarks section says nothing about integrity levels — and
  it *does* document the F12, cross-thread-hWnd and already-registered failure modes, so the omission
  carries some weight.

**The user-visible symptom, if it does occur:** the hotkey registers successfully (no error,
`isEnabled` true, settings UI correct), and pressing it while an elevated window has focus does
**nothing at all** — no event, no log line, no failure — resuming the instant a normal window is
focused. There is no callback and no API to detect the condition. That silence is what makes it a
support nightmare regardless of which mechanism causes it.

**Does running Aloud elevated fix it, and is that a bad trade?** It removes the UIPI asymmetry, yes.
It is a bad trade, and every leg is verifiable:
- **VERIFIED — UAC prompt every launch.** `requestedExecutionLevel level="requireAdministrator"`:
  "the system prompts for credentials."
  ([Application manifests](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests))
- **VERIFIED — and this is decisive — it breaks launch-at-login entirely.** "Applications that
  require administrator-level privileges to run are **blocked** when launched from: Per-user startup
  folder, Per-machine startup folder, Per-user RUN key, Per-machine RUN key… Windows Vista **blocks**
  applications that require elevation in the user logon path."
  ([UAC and the logon path](https://learn.microsoft.com/en-us/previous-versions/bb325654(v=msdn.10)))
  Elevating Aloud kills §2 unless you also move to a Task Scheduler task with
  `TASK_RUNLEVEL_HIGHEST` — which can only be registered from an already-elevated process. A
  two-mechanism rewrite to buy one edge case.
- **VERIFIED — drag-and-drop from Explorer breaks** (the entire subject of Microsoft's MIC/UIPI
  archive post).
- **VERIFIED — the `uiAccess="true"` escape hatch is unavailable.** It requires the binary to be
  **digitally signed** and installed in a **secure location** (`%SystemRoot%`, `%ProgramFiles%`).
  Aloud is unsigned and installs per-user to `%LOCALAPPDATA%`. Off the table.

**Verdict: the policy is right, the premise is not yet established.** Keep "unfixable — document it,
do not chase it" as the *conclusion*, but demote the premise from "known OS behaviour" to a **test
item** (question 24). If the test shows the hotkey works against an elevated window, delete the
landmine from `M3-carry-forward.md` rather than propagating it.

### 3.4 Capturing a chord in a WebView2 settings UI

- **VERIFIED — capture `event.code`, never `event.key`.** W3C UI Events: "`code` values are based
  only on the key's **physical location** on the keyboard and **do not vary based on the user's
  current locale**", whereas `key` reflects the character the active layout produces. The spec's own
  example: "The `Digit2` and `KeyQ` keys… generate `2` and `q` when the US locale is active and `é`
  and `a` when the French locale is active." ([UI Events KeyboardEvent code](https://www.w3.org/TR/uievents-code/))
  **This is not a preference** — Andrii runs German and Ukrainian layouts, and on German QWERTZ the
  physical `Y` key reports `code: "KeyZ"` / `key: "z"`. Only `code` survives a layout switch.
- **VERIFIED — it maps 1:1 onto the Rust side.** `global-hotkey` re-exports
  `keyboard_types::{Code, Modifiers}`, and `keyboard_types::Code` **is** the W3C `code` enumeration.
  A JS capture UI can emit `"control+alt+KeyS"` and Rust parses it directly. No translation table.
- **LIKELY — modifiers** come from `keydown`'s `ctrlKey` / `altKey` / `shiftKey` / `metaKey`;
  `metaKey` is the Windows key.
- **LIKELY — dead keys are a non-problem** *because* you capture `code`. On German layouts `´`, `` ` ``
  and `^` report `key: "Dead"` but `code` is still the physical key (`Backquote`, `BracketRight`…).
  Worth one manual check (question 28).
- **LIKELY/UNVERIFIED — chords a WebView2 `keydown` handler cannot see.** The WebView only receives
  input the OS routes to its window; anything consumed above it never arrives: `Win+<anything>` the
  shell claims, `Ctrl+Alt+Del`, `Alt+Tab`, `Win+L`, `F12`, and chords another app already claimed via
  `RegisterHotKey`. **Design consequence: the capture UI cannot distinguish "you pressed nothing" from
  "the OS ate it."** It needs an explicit timeout plus a "that combination is reserved by Windows"
  message rather than sitting silent.
- **VERIFIED — WebView2's own accelerator layer, if a chord you want is being swallowed by Edge.**
  `ICoreWebView2Controller::add_AcceleratorKeyPressed`: "A key is considered an accelerator if either
  of the following conditions are true: **Ctrl or Alt is currently being held**; **The pressed key
  does not map to a character**… The `Escape` key is always considered an accelerator."
  `put_Handled(TRUE)` prevents the default action. It surfaces the raw **Win32 virtual key code**, not
  a `code` string — so it is the wrong layer for chord capture, the right layer for rescuing `Ctrl+P`
  / `Ctrl+F`. ⚠ In windowed mode the handler runs **synchronously and blocks the browser process**.
  ([AcceleratorKeyPressed](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2acceleratorkeypressedeventargs))
- **A low-level hook is NOT needed for a chord picker.** A DOM `keydown` in the focused settings
  window gives physical code + modifier flags — everything a picker needs. `WH_KEYBOARD_LL` only buys
  capturing chords the OS consumes and *overriding* OS hotkeys, which is what `global-hotkey` issue
  **#161** (open, 2025-08-28) proposes. Out of scope.
- **If a hook were used anyway — three hard facts, all VERIFIED**
  ([SetWindowsHookEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowshookexw),
  [LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)):
  - **No special privilege required.** `WH_KEYBOARD_LL` needs no elevation, no DLL, and is "not
    injected into another process" — but the installing thread **must pump messages**.
  - **It is silently removed if slow.** "If the hook procedure times out… **on Windows 7 and later,
    the hook is silently removed without being called. There is no way for the application to know
    whether the hook is removed.**" Timeout is `LowLevelHooksTimeout` under `HKCU\Control Panel\Desktop`,
    capped at 1000 ms since Win10 1709. For an app that may do work on the hotkey, a real footgun.
  - **Microsoft steers you away:** "In most cases where the application needs to use low level hooks,
    it should monitor **raw input** instead."
  - **AV risk is real but conditional.** Microsoft's own UIPI documentation frames `SetWindowsHookEx`
    in keylogger terms. An **unsigned** binary that installs a global keyboard hook **and** writes a
    Run key is the classic infostealer signature triad. **LIKELY**, not VERIFIED — no vendor statement
    names it as a detection rule. The mitigation is the same either way: don't use the hook, sign the
    binary.

### 3.5 `tauri-plugin-global-shortcut` on Windows — from source

Plugin `2.3.2` → `global-hotkey = "0.8"`. Read from `plugins-workspace/plugins/global-shortcut/` and
`tauri-apps/global-hotkey`.

- **VERIFIED — runtime unregister/re-register is fully supported.** `register`, `unregister`,
  `unregister_multiple`, `unregister_all`, plus the matching JS commands. A rebinding UI is a
  first-class use case, which is exactly what Andrii asked for.
- **VERIFIED — registration failures surface as `Result::Err`, and "already taken" is typed:**
  ```rust
  let result = RegisterHotKey(self.hwnd, hotkey.id() as _, mods, vk_code as _);
  if result == 0 {
      let error = std::io::Error::last_os_error();
      if let Ok(ERROR_HOTKEY_ALREADY_REGISTERED) = WIN32_ERROR::try_from(raw_os_error) {
          return Err(crate::Error::AlreadyRegistered(hotkey));
      }
      return Err(crate::Error::OsError(error));
  }
  ```
  The plugin propagates via `?` into `plugin::Error::GlobalHotkey(String)`. **Nothing is swallowed.**
  ⚠ But the typed variant is **flattened to a string** at the plugin boundary — JS gets a rejected
  promise with the text `HotKey already registered: HotKey { … }` and can only substring-match. Plan
  the settings-UI error copy around that.
- **VERIFIED — `is_registered()` is not what its name suggests.** Its own doc comment: "Determines
  whether the given shortcut is registered **by this application** or not. **If the shortcut is
  registered by another application, it will still return `false`.**" It is a lookup in the plugin's
  own `HashMap`. **The only way to learn a chord is taken system-wide is to attempt registration and
  read the error.** A settings UI that uses `is_registered()` as a conflict check will report every
  conflicting chord as free.
- **VERIFIED — threading.** `GlobalHotKeyManager::new()` creates a hidden
  `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED` message window;
  `WM_HOTKEY` lands on its window proc. The plugin marshals every register/unregister through
  `app.run_on_main_thread()`, with `unsafe impl Send/Sync` justified by that invariant. Calling these
  off the main thread without the plugin's wrapper breaks the safety assumption.
- **VERIFIED — chord string round-trip.** `HotKey::into_string()` emits, in fixed order, `shift+`,
  `control+`, `alt+`, `super+`, then `Code::to_string()` (the W3C code, e.g. `KeyS`). Parsing requires
  **modifiers first, exactly one main key** — `"shift+alt+KeyQ"` legal, `"shift+KeyQ+alt"` errors.
  `serde` support is on. `CMD_OR_CTRL` = `SUPER` on macOS, `CONTROL` elsewhere; `Modifiers::META` is
  normalised to `SUPER`.
- **VERIFIED — Windows specifics baked in.** `MOD_NOREPEAT` is **always** OR-ed in (no auto-repeat,
  ever). Release detection spawns a **thread per keypress** polling `GetAsyncKeyState` every 50 ms
  (added in #178 after #176, "Windows hold-to-talk hotkeys busy-spin"). Relevant only if Aloud ever
  wants push-to-talk.
- **Open Windows-relevant issues:** `global-hotkey` **#161** (override OS hotkeys, open);
  **#159** `Alt` alone as a key — maintainer: "At least Windows doesn't support keys as keys that are
  also modifiers" (open; **works on macOS**, so a macOS chord may not port); **#179** Ctrl+Pause needs
  `VK_CANCEL`. `plugins-workspace` **#1519** "[global-shortcut] MetaLeft is not valid" — the JS
  binding rejects some `Code` values the Rust docs list (open since 2024-07).

### 3.6 Config and model paths on Windows

- **VERIFIED — Tauri's own resolver.** `app_config_dir()` = `{FOLDERID_RoamingAppData}\<bundle
  identifier>` = `C:\Users\<user>\AppData\Roaming\com.andriileso.aloud`.
  `app_cache_dir()` = `{FOLDERID_LocalAppData}\<identifier>`.
  ([PathResolver](https://docs.rs/tauri/latest/tauri/path/struct.PathResolver.html))
- **VERIFIED — and this resolves `M3-carry-forward.md` landmine 1.** `dirs` 5.0 on Windows collapses
  cache and local-data into the same folder with **no `Caches` subdirectory**: `cache_dir` →
  `{FOLDERID_LocalAppData}`, `data_local_dir` → `{FOLDERID_LocalAppData}`, `config_dir` →
  `{FOLDERID_RoamingAppData}` ([dirs-rs README](https://github.com/dirs-dev/dirs-rs)). So
  `dirs::cache_dir()` on Windows returns **`C:\Users\<user>\AppData\Local` itself** — not
  `…\Local\Caches`, which has no Windows analogue. Aloud's `model_dir()` third branch is
  `dirs::cache_dir()/supertonic3`, which on Windows means the **385 MB model lands directly in
  `%LOCALAPPDATA%\supertonic3`**, outside `%LOCALAPPDATA%\com.andriileso.aloud\`.
- **VERIFIED — and that placement means it is never cleaned up.** Tauri's NSIS uninstaller shows an
  opt-in "Delete app data" checkbox, and only when checked **and** not updating does it run:
  ```nsi
  SetShellVarContext current
  RmDir /r "$APPDATA\${BUNDLEID}"
  RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ```
  It deletes `${BUNDLEID}`-named folders only. **A `dirs::cache_dir()`-derived model directory is
  never removed** — 385 MB orphaned on every uninstall, forever. Config survives every update
  unconditionally, which is the desired behaviour.

### What this means for M6

1. **Capture `event.code`, store `event.code`, and round-trip through `HotKey::into_string()`'s
   format** (`shift+control+alt+super+` then the code). Never `event.key` — the German layout breaks
   it silently.
2. **Never use `is_registered()` as the conflict check.** It only knows about Aloud's own bindings.
   Attempt the registration and parse the error; expect a *string* containing "already registered",
   not a typed variant, at the JS boundary.
3. **Give the capture field a timeout and a "reserved by Windows" message.** A swallowed chord and no
   keypress look identical in the DOM.
4. **Do not default to a `Win+` chord**, and do not attempt `Alt` alone (global-hotkey #159 — works on
   macOS, not on Windows; a macOS chord does not necessarily port).
5. **Do not use `WH_KEYBOARD_LL`.** Not needed for the M4/M6 surface, silently removable on timeout,
   and it is one third of the classic infostealer signature for an unsigned binary.
6. **Do not elevate Aloud.** It would break launch-at-login outright (VERIFIED), and the `uiAccess`
   alternative requires signing plus a secure install location Aloud does not have.
7. **Test the UIPI claim once (question 24) and then correct or confirm
   `M3-carry-forward.md` landmine 3 in the same change set.** Do not propagate an unverified
   limitation into the PC brief as established fact.
8. **Route `model_dir()` through Tauri's `app_cache_dir()` / `app_local_data_dir()` on Windows, not
   raw `dirs::cache_dir()`** — otherwise 385 MB lands in `%LOCALAPPDATA%\supertonic3` and survives
   every uninstall.

---

## 4. Screen capture and OCR

### 4.1 Is there a Windows analogue of macOS TCC "Screen Recording"? — No.

- **VERIFIED — capabilities only bind packaged apps.** "Most scenarios for app capabilities are
  relevant only to apps that have package identity, and that run in an AppContainer."
  ([App capability declarations](https://learn.microsoft.com/en-us/windows/uwp/packaging/app-capability-declarations))
  A non-packaged Win32 app has no package manifest, therefore no capability, therefore no gate.
- **VERIFIED — none of `Windows.Graphics.Capture` (via the Win32 interop factory), GDI `BitBlt`, or
  DXGI Desktop Duplication prompts, requires a Settings toggle, or requires elevation.** Evidence for
  WGC unpackaged: `robmikh/screenshot-rs` is a plain `cargo` binary using the `windows` crate that
  captures a monitor to PNG, and its repo tree contains **no `.manifest`, no `.appxmanifest`, no
  packaging project** (`gh api repos/robmikh/screenshot-rs/git/trees/HEAD?recursive=1`).
  `robmikh/Win32CaptureSample`'s README states the only requirement is Windows 10 build 17134.
  [Desktop Duplication](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)
  has no permissions section at all.

**This deletes an entire class of macOS pain.** The `CGPreflightScreenCaptureAccess` false-negative
bug, the "grant died on rebuild" bug, and the restart-after-granting problem have **no Windows
equivalent for capture**. Do not port that logic. (The signing-identity concern reappears elsewhere
— see §1.2 and §5 — just not here.)

- **LIKELY — the one thing that silently censors all three equally.**
  `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` (Win10 2004+) is enforced in DWM, so
  protected windows return **black** under BitBlt, Desktop Duplication *and* WGC alike
  ([SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
  plus secondary write-ups). Not a permission — a content restriction. Aloud would OCR a black
  rectangle over a DRM or banking window and return nothing; it should say that rather than go quiet.

### 4.2 The yellow border, and the picker

- **VERIFIED — the picker is NOT mandatory.** `IGraphicsCaptureItemInterop::CreateForWindow(HWND)` /
  `CreateForMonitor(HMONITOR)`, minimum Windows 10 1903 (build 18362), header
  `windows.graphics.capture.interop.h`
  ([interop reference](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nn-windows-graphics-capture-interop-igraphicscaptureiteminterop)).
  `screenshot-rs` calls exactly this from Rust. The `graphicsCapture` capability is required **only**
  for `GraphicsCapturePicker` — which a crosshair-drag UX would never use.
- **VERIFIED — the border is real and system-drawn.** "a yellow notification border is drawn by the
  system around the actively captured item."
  ([Screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture))
- **VERIFIED — and it cannot be turned off from a non-packaged app.**
  `GraphicsCaptureSession.IsBorderRequired` (Windows 10 build 20348, UniversalApiContract v12.0)
  remarks, verbatim: "your app must get consent from the user by calling
  `GraphicsCaptureAccess.RequestAccessAsync`… passing in the value
  `GraphicsCaptureAccessKind.Borderless`… If the user denies access, setting this property to false
  will succeed, **but the value will be ignored**… To call `RequestAccessAsync` with
  `GraphicsCaptureAccessKind.Borderless`, you must declare the **`graphicsCaptureWithoutBorder`
  capability in your app's package manifest**."
  A non-packaged app has no package manifest → the property silently no-ops. **Constraint conflict —
  see below.**
- **VERIFIED — a trap to avoid.** `GraphicsCaptureItem.TryCreateFromDisplayId` /
  `TryCreateFromWindowId` (the *WinRT* creation path, also 20348/v12.0) require
  `RequestAccessAsync(Programmatic)` and the `graphicsCaptureProgrammatic` capability **in a package
  manifest**. They look like the clean modern API and they hard-fail unpackaged. Use the Win32
  interop factory.
- **VERIFIED — `IsCursorCaptureEnabled`** (Windows 10 2004 / SDK 19041) is a plain property, no
  capability, no prompt. Set it `false`.
- **LIKELY — Windows 11 24H2 added a user-facing "Screen capture border" toggle** under System →
  Display → Graphics. Secondary sources only (windowsreport, obs-versions.com); no Microsoft primary
  source found. It is a *user* setting; the app cannot set it.

### 4.3 Which capture API for "one rectangle of the virtual desktop, once"

A genuine trade-off, presented rather than decided:

| | Border | Works unpackaged | One-shot cost | Failure modes |
|---|---|---|---|---|
| **GDI `BitBlt`** from a screen DC | none (border is a WGC feature) | yes | lowest — one call, spans the whole virtual desktop including negative coords | black on `WDA_EXCLUDEFROMCAPTURE`; slower per-pixel; no HDR |
| **Windows.Graphics.Capture** | **yellow border, undisableable unpackaged** | yes (interop path) | frame pool + await ≥1 `FrameArrived`; per-monitor item, must crop | needs a D3D11 device; HDR needs `R16G16B16A16_FLOAT` |
| **DXGI Desktop Duplication** | none | yes | worst fit — built for frame-by-frame streaming; `AcquireNextFrame` loop + `DXGI_ERROR_ACCESS_LOST` handling | **VERIFIED** per-output only, and "the surface … is always in the un-rotated orientation" — you must rotate manually |

- **The honest match for a single still frame is `BitBlt`**: zero border, zero packaging pressure,
  one call, and it natively spans the virtual desktop including negative coordinates. WGC is the more
  modern path (native `IDirect3DSurface` → `SoftwareBitmap`, HDR-capable) but buys the yellow border.
  Desktop Duplication is documented as "frame-by-frame updates to the desktop" for "remote access …
  collaboration scenarios" — its rotation handling and access-lost recovery are pure cost here.
  **Owner call, not mine.**

### 4.4 Per-monitor DPI and the virtual-desktop coordinate space

- **VERIFIED — `PerMonitorV2` is expressible ONLY via `<dpiAwareness>`** (Windows 10 1607+); the
  legacy `<dpiAware>` tag has "Not supported" in the PerMonitorV2 row, and on 1607+ `<dpiAwareness>`
  wins when both are present.
  ([Setting the default DPI awareness](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process))
  See §6.2 for the ordering rule and what Tauri/tao actually do (API call, no manifest).
- **VERIFIED — negative coordinates are normal and must be designed for.** "The primary monitor
  contains the origin (0,0)… When the primary monitor is not in the upper left of the virtual screen,
  parts of the virtual screen have negative coordinates. Because the arrangement of monitors is set
  by the user, all applications should be designed to work with negative coordinates."
  ([The virtual screen](https://learn.microsoft.com/en-us/windows/win32/gdi/the-virtual-screen))
  Bounds are 16-bit-signed-limited.
- **LIKELY — the API → value mapping.** `GetSystemMetrics(SM_XVIRTUALSCREEN / SM_YVIRTUALSCREEN /
  SM_CXVIRTUALSCREEN / SM_CYVIRTUALSCREEN)` gives the virtual-screen origin and extent in **physical
  pixels once PerMonitorV2 is set**; `EnumDisplayMonitors` + `GetMonitorInfo` give per-monitor
  `rcMonitor` in the same space; `GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI)` gives the DPI, and
  scale = dpi / 96.0 (tao's `dpi_to_scale_factor` is literally `dpi / USER_DEFAULT_SCREEN_DPI`).
- **UNVERIFIED — exactly what breaks when awareness is not set.** The expectation is that Windows
  virtualises the coordinate space: `GetSystemMetrics` returns *logical* pixels scaled to the primary
  monitor's DPI, so a rectangle dragged on a 150% secondary monitor maps to the wrong physical pixels,
  and windows are bitmap-stretched (blurry). The docs confirm only that the modes "can use different
  coordinate spaces." **Must be tested on the PC** — this is precisely the pot-app #1161 bug class
  that hard constraint 4 exists to prevent.

### 4.5 OCR — `Windows.Media.Ocr`

**The engine is inbox. The language models are not.** This is where the zero-dependency constraint
takes its real damage.

- **VERIFIED — the engine ships with every Windows 10/11 SKU.** `OcrEngine` is Windows 10
  10.0.10240.0, UniversalApiContract v1.0, implemented in `windows.dll`. No Windows App SDK, no
  redistributable. ([OcrEngine](https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine))
- **VERIFIED — the languages are Features-on-Demand the user must install.** "A language pack must
  be installed on the device to be used. A user can install new OCR language packs through Windows
  Settings."
  ([AvailableRecognizerLanguages](https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine.availablerecognizerlanguages))
  The capability is `Language.OCR~~~<tag>~0.0.1.0`, package
  `Microsoft-Windows-LanguageFeatures-OCR-<tag>-Package`, and it **depends on `Language.Basic` of the
  same language**
  ([Features on Demand — language FOD](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/features-on-demand-language-fod)).
- **VERIFIED — what a stock en-US install actually has.** Microsoft's own PowerToys doc shows the
  real shape: `Language.OCR~~~en-US~0.0.1.0 State : Installed`, while `el-GR`, `en-GB`, `es-ES`,
  `es-MX` are all `State : NotPresent`. **You get exactly the languages whose display-language
  features are installed, and nothing else.**
- **VERIFIED — edition and privilege notes.** Home editions are fine, with one exception: "You cannot
  add languages to Home Single Language and Home Country Specific editions."
  ([Available language packs](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/available-language-packs-for-windows)).
  Windows 11 relaxed the privilege: "Starting in Windows 11, standard users can acquire Language
  Feature-on-Demand packages from Time & Language page in the Settings app." Admins can block this
  entirely via the `RestrictLanguagePacksAndFeaturesInstall` policy.
- **VERIFIED — the API failure mode is `null`, not an exception.**
  `TryCreateFromLanguage(Language)` → "If the specified language can be resolved… returns new
  instance of OcrEngine class, **otherwise returns null**."
  `TryCreateFromUserProfileLanguages()` → same, resolved against `GlobalizationPreferences.Languages`,
  **otherwise null**. `AvailableRecognizerLanguages` returns an empty list rather than throwing.
  `IsLanguageSupported` uses BCP-47 *matching*, not exact equality. So a missing language is a silent
  `null` — exactly the shape of failure this research exists to pre-empt.
- **LIKELY — Ukrainian OCR does not exist on Windows, at any price.** Microsoft Q&A, question "UWP
  OcrEngine does not support Ukrainian language", accepted answer: the Windows OCR engine supports 25
  languages including Russian and Serbian, "and unfortunately, Ukrainian is not included"
  ([Q&A 5524397](https://learn.microsoft.com/en-us/answers/questions/5524397/uwp-ocrengine-does-not-support-ukrainian-language)).
  Tagged LIKELY, not VERIFIED: this is a support-agent post on a page carrying an AI disclaimer, not
  an API reference. Note that `uk-UA` **is** a shipping Windows *display* language pack — a display
  pack existing does not imply an OCR FOD exists. Authoritative check is `Get-WindowsCapability` on
  the PC (question 16).
- **LIKELY — of Aloud's four target languages:** English present by default on a stock en-US box;
  German and Russian in the supported set but installed only if the user added those language
  features; Ukrainian unavailable.
- **Downstream consequence the plan must absorb:** `lingua-rs` runs *after* OCR, so it can only
  classify text the OCR already transcribed. Ukrainian text pushed through the Russian recognizer will
  mangle the Ukrainian-only glyphs (і, ї, є, ґ) **before** lingua ever sees it. The four-language
  auto-detect design is a three-language design on Windows.
- **VERIFIED — what the user-facing install step actually is.** Microsoft's own instructions
  ([PowerToys Text Extractor](https://learn.microsoft.com/en-us/windows/powertoys/text-extractor)):
  open PowerShell **as Administrator**, `Get-WindowsCapability -Online | Where-Object { $_.Name -Like
  'Language.OCR*' }`, then `$Capability | Add-WindowsCapability -Online` — "may take several minutes
  to complete." On Windows 11 a standard user can do the equivalent through Settings.
- **VERIFIED — the newer Microsoft OCR path is hardware-locked and irrelevant here.** Windows App SDK
  / Windows AI `Microsoft.Windows.AI.Imaging.TextRecognizer`: these "run **exclusively on devices with
  a neural processing unit (NPU)**, making them faster and more accurate than the legacy
  Windows.Media.Ocr.OcrEngine APIs."
  ([Text recognition](https://learn.microsoft.com/en-us/windows/ai/apis/text-recognition))
  The target PC is an RTX 2070 Super desktop with no NPU. It would also add two banned dependencies:
  the Windows App Runtime, and a model downloaded at runtime via `EnsureReadyAsync()`. **Rule it out;
  do not revisit.**

### 4.6 Calling WinRT from Rust — the `windows` crate

- **VERIFIED — windows-rs does NOT initialise COM for you.** microsoft/windows-rs issue
  [#1169](https://github.com/microsoft/windows-rs/issues/1169): "The library doesn't appear to call
  `CoInitialize` (or `RoInitialize`) anywhere, so it appears to rely on the automatic
  initialization." That issue documents a real **segfault** when another crate called
  `CoUninitialize` and unloaded COM DLLs out from under a WinRT thread. Initialise explicitly, and be
  wary of any other COM-using crate entering the dependency tree.
- **VERIFIED — the reference pattern for exactly this workload.** `robmikh/screenshot-rs/src/main.rs`:
  `RoInitialize(RO_INIT_MULTITHREADED)?` via
  `windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED}`. WinRT wants `RoInitialize`,
  not `CoInitializeEx`.
- **VERIFIED — neither API imposes an apartment.** Both `OcrEngine` and `GraphicsCaptureSession`
  carry `[Threading(ThreadingModel.Both)]` + `[MarshalingBehavior(MarshalingType.Agile)]` in their
  class metadata — callable from STA or MTA.
- **LIKELY — the real threading constraint is the frame pool, not the apartment.**
  `Direct3D11CaptureFramePool.Create` depends on a `Windows.System.DispatcherQueue` on the calling
  thread; `CreateFreeThreaded` removes that dependency and raises `FrameArrived` on the pool's own
  worker thread. A Tauri main thread has a Win32 message pump but no WinRT `DispatcherQueue` by
  default. `screenshot-rs` uses `CreateFreeThreaded` — copy that.
- **VERIFIED — package identity is NOT required for either API.** `Windows.Graphics.Capture` works
  unpackaged via the interop factory (`robmikh/screenshot-rs`, `robmikh/Win32CaptureSample`, neither
  packaged). **LIKELY (strong)** for OCR: PowerToys' `PowerOCR` calls
  `OcrEngine.TryCreateFromLanguage` → `RecognizeAsync(softwareBmp)`
  (`src/modules/PowerOCR/PowerOCR/Helpers/OcrExtensions.cs:104-105`) and PowerToys ships as a
  non-MSIX `PowerToysSetup-*.exe`. No OCR API carries a "requires package identity" note.
  **What genuinely requires identity is avoidable**: `graphicsCapture` (picker only),
  `graphicsCaptureWithoutBorder` (border only), `graphicsCaptureProgrammatic`
  (`TryCreateFromDisplayId`/`WindowId` only).
  **Verdict: stay non-packaged.** The only thing MSIX buys is turning off the yellow border.
- **VERIFIED — the exact `windows` crate features** (from `crates/libs/windows/Cargo.toml` plus
  `screenshot-rs/Cargo.toml`):
  ```
  Foundation, Graphics, Graphics_Capture, Graphics_DirectX, Graphics_DirectX_Direct3D11,
  Graphics_Imaging, Media, Media_Ocr, Globalization,
  Win32_Foundation, Win32_Graphics_Direct3D, Win32_Graphics_Direct3D11,
  Win32_Graphics_Dxgi, Win32_Graphics_Dxgi_Common, Win32_Graphics_Gdi,
  Win32_System_Com, Win32_System_WinRT, Win32_System_WinRT_Direct3D11,
  Win32_System_WinRT_Graphics_Capture, Win32_UI_WindowsAndMessaging
  ```
  `Media_Ocr = ["Media"]`; `Graphics_Imaging = ["Graphics"]` (for `SoftwareBitmap`); `Globalization`
  for `Windows.Globalization.Language`. `Win32_Graphics_Imaging` is WIC (PNG encoding) — a different
  thing, needed only to save files.
- **VERIFIED — no Windows SDK needed at build time.** The `windows` crate ships pre-generated
  bindings, and `windows-link` "Links C-style functions without import libraries" (windows-rs README).
  An MSVC target and linker are still required, so the build happens on the PC regardless.
- **VERIFIED — frame → OCR path.** `Direct3D11CaptureFrame.Surface` (`IDirect3DSurface`) →
  `SoftwareBitmap.CreateCopyFromSurfaceAsync(surface[, BitmapAlphaMode])` (both overloads do "a deep
  copy") → `OcrEngine.RecognizeAsync(bitmap)`. SDR frame-pool format is
  `DirectXPixelFormat::B8G8R8A8UIntNormalized`; HDR displays need `R16G16B16A16_FLOAT` throughout
  "to avoid pixel overclipping (i.e. the captured content looks washed out)."
- **LIKELY — OCR wants `BitmapPixelFormat.Bgra8` + `BitmapAlphaMode.Premultiplied`.** That is what
  Microsoft's own UWP OCR sample decodes to. The `RecognizeAsync` reference page has **no remarks
  section at all** and states no format requirement, so this is convention, not contract. Confirm on
  the PC.
- **VERIFIED — clamp to `OcrEngine.MaxImageDimension`, and note the opposite problem.** PowerToys
  does `if (bmp.Width * 1.5 > OcrEngine.MaxImageDimension)` before **upscaling** by 1.5×
  (`OcrExtensions.cs:77`) — because Windows OCR returns nothing on images that are too small. For a
  drag-a-small-rectangle UX that is not an edge case, it is the common case.

### What this means for M6

1. **Delete the macOS permission model from the Windows plan.** No TCC analogue exists for capture.
   No preflight, no restart-after-grant, no grant-invalidated-by-rebuild. Do not port that code, and
   do not let a `#[cfg]` for it leak into pipeline logic (hard constraint 8).
2. **Use the Win32 interop factory (`CreateForMonitor` / `CreateForWindow`) if using WGC at all —
   never `GraphicsCapturePicker`, never `TryCreateFromDisplayId`/`WindowId`.** The latter two require
   package identity and will hard-fail unpackaged.
3. **Decide BitBlt vs WGC as an owner call before writing the selector** (see Constraint conflicts).
   The yellow border is the whole decision, and it is not fixable in code while non-packaged.
4. **Drive the OCR language UI from `AvailableRecognizerLanguages`, never from a hardcoded list.**
   `TryCreateFromLanguage` returns `null` for a missing language — a silent failure. Default via
   `TryCreateFromUserProfileLanguages()`.
5. **Plan for Ukrainian OCR being absent on Windows.** Either accept three-language coverage there,
   or reopen the bundled-OCR question. Do not let `lingua-rs` mask it — lingua runs after OCR and
   cannot recover glyphs the recognizer never produced.
6. **Clamp to `MaxImageDimension` and upscale small regions ~1.5×** before calling `RecognizeAsync`.
   Small-region drag is Aloud's primary gesture and it is the case Windows OCR fails on.
7. **Call `RoInitialize(RO_INIT_MULTITHREADED)` explicitly**, use
   `Direct3D11CaptureFramePool::CreateFreeThreaded`, and set `IsCursorCaptureEnabled = false`.
8. **Stay non-packaged.** Both APIs work; MSIX buys only the border.

---

## 5. Packaging and distribution

### 5.1 What Tauri 2 actually supports on Windows

- **VERIFIED — exactly two Windows targets: `msi` and `nsis`.** `BundleType` is
  `Deb | Rpm | AppImage | Msi | Nsis | App | Dmg`; anything else errors `unknown bundle target`
  (`crates/tauri-utils/src/config.rs:132-205`).
- **VERIFIED — the toolchain versions are pinned in the bundler source**: WiX **v3.14.1 RTM**
  (SHA256-pinned) and NSIS **3.11** (SHA1-pinned) plus `nsis_tauri_utils.dll` v0.5.3
  (`crates/tauri-bundler/src/bundle/windows/msi/mod.rs:39-41`, `nsis/mod.rs:38-44`). WiX 3, not 4/5.
  MSI additionally needs the **VBSCRIPT** optional Windows feature enabled or `light.exe` fails.
- **VERIFIED — Tauri 2 cannot produce MSIX or APPX.** Tracking issue **#4818 open since 2022-08-01**;
  **#8548 closed `not_planned`**. Maintainer, 2026-01-25: no explicit plans, not a priority, no ETA
  (possibly v3, itself undated). Nor is there a portable-exe target — `target/release/aloud.exe` runs,
  but you would hand-assemble a portable bundle exactly as `packaging/make-app.sh` does on macOS.
- **VERIFIED — the first packaging run needs network, for a third reason.** The NSIS toolchain is
  downloaded and hash-verified at bundle time into `%LOCALAPPDATA%\tauri\NSIS` (same pattern for WiX),
  and WebView2 is a third build-time download unless `FixedRuntime` is chosen. So the PC's first
  `tauri build` needs the network for **NSIS/WiX + WebView2 + `ort-sys`** independently. All three
  then cache to disk.

### 5.2 Format comparison against Aloud's actual needs

| | **NSIS** | **MSI (WiX 3.14.1)** | **MSIX** | **Portable .exe** |
|---|---|---|---|---|
| Tauri 2 native | yes | yes | **no** | no (hand-assembled) |
| Sidecar helper binary | yes (`externalBin`) | yes | yes, hand-built | yes |
| Elevation to install | **no** (default) | **yes** | no (the cert is the problem) | no |
| Per-user install | **yes, default** | **no** | yes | yes |
| Writes `%APPDATA%` at runtime | unrestricted | unrestricted | ⚠ possibly virtualized | unrestricted |
| 385 MB first-run download | fine | fine | fine (no size rule) | fine |
| Startup entry | HKCU Run | HKCU Run | ⚠ Run key likely virtualized | HKCU Run |

- **VERIFIED — NSIS `installMode` defaults to `currentUser`**: installs to a directory that "doesn't
  require Administrator access", metadata under HKCU. `perMachine` needs admin, and **`both` requires
  admin even when the user picks current-user** (`config.rs:818-846`).
- **VERIFIED — MSI is per-machine only.** Tauri's WiX template hardcodes `InstallScope="perMachine"`
  and installs into `ProgramFiles64Folder` (`msi/main.wxs:28,121`). There is no per-user MSI option.
- **VERIFIED — sidecar naming.** `externalBin` entries must carry the target-triple suffix on
  Windows: `aloud-helper-x86_64-pc-windows-msvc.exe`.

### 5.3 MSIX specifics — and why it is a dead end here

- **VERIFIED — package identity is a 5-tuple** whose Publisher field is "the app developer's subject
  name as identified by their signing certificate". `PackageFamilyName = <Name>_<PublisherId>`.
  **Changing signing certificates changes the family name, so Windows treats it as a different app**:
  new data location, no in-place upgrade.
- **UNVERIFIED — whether `%APPDATA%` writes are virtualized for a full-trust packaged app.**
  Microsoft's own current docs contradict each other: the packaging-prereqs page says AppData "is
  redirected"; the containerization overview (updated 2026-04-15) says redirection applies "for
  AppContainer apps, **or pass through for full-trust apps**". A 2025-09-09 commit to a third page
  deleted "an incorrect statement of what causes I/O virtualization". Would have to be measured.
- **LIKELY — when redirection is in effect**, writes land under
  `…\AppData\Local\Packages\<PackageFamilyName>\LocalCache\Local\…` (two independent reports from
  packaged Tauri apps, [tauri#9936](https://github.com/tauri-apps/tauri/issues/9936)).
- **VERIFIED — the MSIX install directory is read-only, unconditionally**, and uninstall wipes
  everything including redirected AppData/registry writes — so a reinstall re-downloads the 385 MB.
- **VERIFIED — 385 MB itself is fine under MSIX.** The local app data store has "no general size
  restriction… Use the local app data store for… large data sets", and persists through updates. (Do
  **not** use `TemporaryFolder` — "The System Maintenance task can automatically delete data stored at
  this location at any time.") The 25 GB cap is a Store *submission* limit on package size.
- **VERIFIED — a Run-key startup entry probably would not work under MSIX.** HKCU writes go "to a
  separate per-app, per-user private hive… **Keys in the virtualized hive are only visible to the
  app**", so Explorer would never enumerate it. Opting out requires the `unvirtualizedResources`
  **restricted capability**, which Microsoft says "is currently intended to be used only by certain
  types of desktop PC games that are published by Microsoft and our partners." MSIX's own
  `windows.startupTask` is the correct mechanism there (see §2.1(d)).
- **VERIFIED — sideloading requires a *trusted* certificate, not merely a signature.** "the package
  doesn't just have to be signed but also trusted on the device… the certificate has to chain to one
  of the trusted roots on the device." Self-signed requires the user to import the cert into **Trusted
  People**, and Microsoft's documented procedure routes through Local Machine and "might require an
  administrator to elevate."
- **VERIFIED — Store distribution is the one case where MSIX gets cheap.** "The Microsoft Store will
  automatically re-sign your MSIX/AppX packages with a Microsoft certificate… You don't need to
  purchase a CA-trusted code signing certificate." Also **VERIFIED**: `ms-appinstaller:` one-click web
  install "is disabled by default as of December 2023."

### 5.4 Code signing

**Unsigned behaviour — VERIFIED, from Microsoft's own table**
([SmartScreen and app reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)):

| Signing state | Outcome |
|---|---|
| No signature | Warning — "Windows protected your PC"; user must choose "Run anyway". Enterprise policy can prevent continuation entirely. |
| **Self-signed certificate** | **Warning — same behavior as no signature** |
| Valid OV/EV certificate | Warning — app flagged as unrecognized **until reputation accumulates** |

- **VERIFIED — warn-and-bypass by default.** "By default, Microsoft Defender SmartScreen allows users
  to bypass warnings." It becomes unbypassable only under GPO "Warn and prevent bypass" — managed
  machines.
- **VERIFIED — Mark-of-the-Web** is the `Zone.Identifier:$DATA` NTFS alternate data stream
  (`ZoneId=3`), applied only for Internet/Restricted zones and **lost entirely on FAT32/exFAT**.
  SmartScreen app reputation is scoped to internet-sourced files: "It doesn't protect against
  malicious files on internal locations or network shares."
  **LIKELY** that Explorer's ZIP extractor propagates MOTW on NTFS (Eric Lawrence, ex-IE/Edge security
  PM); third-party archivers historically varied. No Microsoft primary source found.
- **VERIFIED — Smart App Control is the one that can hard-block.** "Malware, Potentially Unwanted Apps
  (PUA), and **unknown, unsigned code are blocked by default**", with **no per-app bypass**, and
  crucially **not MOTW-gated**: "Smart App Control signature checks apply to all executable files, not
  just those downloaded from the Internet." Mitigations: it exists only on a clean install of Win11
  22572+, and "If we detect that you're one of those users [developers], we automatically turn Smart
  App Control off."

**Reputation model — and the finding that changes the cost calculus**

- **VERIFIED — reputation attaches to BOTH the certificate and the file hash.** "SmartScreen evaluates
  two signals… 1. **Publisher reputation** — Is the file signed?… 2. **File hash reputation** — Has
  this specific file been downloaded by users without indications of malicious behavior?"
- **VERIFIED — EV no longer buys instant reputation, in Microsoft's own words.** "EV certificates no
  longer bypass SmartScreen… this behavior no longer exists… **Paying a premium for EV solely to avoid
  SmartScreen warnings is no longer justified.**" Removed in **2024**. **This retires the standard
  internet advice to buy EV**, and it is the single most money-relevant finding in this section.
- **VERIFIED — what signing does buy is cross-version carry-over.** "Reputation cannot transfer from
  previous versions unless both were signed using the same publisher identity… **Unsigned files must
  build reputation anew with every update.**" Clearing takes "several weeks and hundreds of clean
  installs", with no manual submission path for consumers.

**CA/Browser Forum requirements, current as of 2026**

- **VERIFIED — current version is CSCBR v3.11.0, effective 16 June 2026**
  ([PDF](https://cabforum.org/uploads/CA-Browser-Forum-CSCBR-3.11.0.pdf)). The hardware rule is still
  normative, §6.2.7.4.1: "Effective June 1, 2023, Subscriber Private Keys for Code Signing
  Certificates SHALL be protected… in a Hardware Crypto Module… FIPS 140-2 Level 2 or Common Criteria
  EAL 4+", with three permitted options (hardware module, cloud key generation/protection, or a
  Signing Service) and §6.2.7.4.2 attestation that the key "was generated in a **non-exportable**
  way". **Plain `.pfx` is dead.**
- **VERIFIED — a newer rule that matters for budgeting: §6.3.2.** Certificates issued on or after
  **1 March 2026** have a maximum validity of **460 days** (was 39 months). Budget ~15-month renewals.
- **VERIFIED — an individual (natural person) CAN get non-EV code signing.** The BRs define
  "Individual Applicant… a natural person… the Applicant's legal name as the Certificate's Subject",
  with §3.2.3 government-photo-ID plus video or in-person verification. **EV requires an entity**
  (§4.1.1), though "Business Entity" explicitly includes sole proprietorships verifiable with a
  Registration Agency. **UNVERIFIED — whether a German Gewerbeanmeldung satisfies a given CA's
  Business Entity test**; that needs a written answer from a specific CA, and is a Rektor-adjacent
  question, not a technical one.
- **LIKELY — cloud signing avoids shipping a hardware token to Berlin**: SSL.com eSigner ("No hardware
  token required"), DigiCert KeyLocker, GlobalSign Atlas, Certum CertManager.
- **Prices.** **VERIFIED as Microsoft's own estimate**: OV "$150–300/year", EV "$400+/year" (a second
  Microsoft page says OV $300–500/year). **LIKELY**: Certum (Polish CA, EU) lists Standard **from
  €139/yr issued to "First and Last Name / Company Name"** — individuals explicitly eligible — and EV
  from €329, company only. **VERIFIED** — free for open source via **SignPath Foundation**, named by
  Microsoft itself: "offers free code signing for qualifying open-source projects… OV-level
  certificate signing through a managed pipeline."

**Microsoft Trusted Signing → renamed Azure Artifact Signing**

- **VERIFIED — the rename** (docs now at `/azure/artifact-signing/`). GA January 2026 is **LIKELY**
  (Microsoft security blog; the docs landing page metadata still says "public preview" —
  internally inconsistent, flagged rather than asserted).
- **VERIFIED — not available to a German individual.** Verbatim: "Public Trust certificates are
  available to organizations in the United States, Canada, the European Union, the United Kingdom…
  **Individual developers must be located in the United States or Canada.**" Corroborated on the
  Windows dev docs: "If you are an individual developer outside those regions, see OV certificates
  below." A German *legal entity* would qualify (EU orgs are in scope), but the MSIX signing page adds
  "Organizations must have a verifiable tax history of **three or more years**."
- **VERIFIED — ~$9.99/month, and a paid Azure subscription is required** ("doesn't support free,
  trial, or sponsored Azure subscriptions"), not pro-rated. Certificates are renewed daily and valid
  72 hours; RFC 3161 timestamping is effectively mandatory; a per-subscriber custom EKU
  (`1.3.6.1.4.1.311.97.<unique>`) is the durable identity anchor since thumbprint pinning cannot
  survive rotation. It "does **not** provide instant SmartScreen trust", and **will never issue EV**.

**Is Windows signing a development-time concern, as it turned out to be on macOS? — Essentially no.**

Each candidate mechanism was checked for a TCC-style binding that would break on rebuild:

- **VERIFIED — Windows Firewall rules are keyed by full file PATH**, not hash, not signature. "You can
  only create rules using the full path to the application(s)". **A rebuild to the same output path
  inherits the rule.**
- **VERIFIED — `RegisterHotKey` has no permission model at all.** The reference documents exactly two
  failure modes (another thread's window; already registered) — no privilege, capability, consent
  prompt, or persisted grant. **Nothing to lose on rebuild.**
- **VERIFIED — screen-capture consent is transient per-pick, not a stored per-app grant.** Consent
  attaches to a `GraphicsCaptureItem` for that run. Picker-less/borderless capture needs *package
  manifest capabilities* — package identity, not a signature. (BitBlt/DXGI: no documented consent
  requirement; argument from absence, **UNVERIFIED**.)
- **VERIFIED — Credential Manager is scoped to the logon session, not the app.** `CredRead` uses "the
  credential set… associated with the **logon session of the current token**." A rebuilt binary reads
  its own credentials fine.
- **VERIFIED — SmartScreen never fires during local development** — a `cargo build` output carries no
  MOTW. The exception is Smart App Control, which is machine state, not a certificate problem.
- **VERIFIED — Controlled Folder Access is path-keyed and off by default.**
- **VERIFIED — the one genuine dev-time signature dependency is `uiAccess="true"`**, which requires
  the app to "Be signed using an Authenticode code signing certificate", be trusted by the system, and
  be installed in a UAC-protected location. Relevant only if hotkeys must work above elevated windows
  (§3.3). A stable self-signed cert trusted on the dev machine would satisfy it for free.

**One reconciliation, because these two findings sit next to each other and could be misread.** §1.2
found a documented case where an Authenticode signature *does* preserve state across a binary move:
the notification-icon `guidItem` registration. That is real, but it is **not** a rebuild-invalidation
problem of the macOS kind (it concerns a *moved* file, not a re-signed one), and it does not apply at
all to Aloud today because `tray-icon` uses `hWnd`+`uID` rather than a GUID. The general verdict
stands: **Windows has no TCC equivalent — buy signing when you ship, not when you build.**

### 5.5 ONNX Runtime / the `ort` crate on Windows

- **VERIFIED — static on Windows too, exactly as on macOS.** `ort-sys/dist.txt` at tag `v2.0.0-rc.7`
  for `x86_64-pc-windows-msvc` resolves to
  `…/ortrs_static-v1.19.2-x86_64-pc-windows-msvc.tgz` (SHA256-pinned) — **`ortrs_static`, not
  `ortrs_dylib`**. `build.rs` then does
  `let static_lib_file_name = if target_os.contains("windows") { "onnxruntime.lib" } else { "libonnxruntime.a" };`
  → `cargo:rustc-link-lib=static=onnxruntime` (`ort-sys/build.rs:447-452`). Maintainer confirms:
  "`onnxruntime` is statically linked by default"
  ([ort#582](https://github.com/pykeio/ort/issues/582)). ONNX Runtime version is pinned to **1.19.2**.
  **The M3 carry-forward's "no dylib to bundle" finding holds on Windows.**
- **VERIFIED — one DLL does appear: `DirectML.dll`, and it is NOT a constraint conflict.** The
  download path unconditionally emits `cargo:rustc-link-lib=dxguid/DXCORE/DXGI/D3D12/DirectML` on
  Windows, and `copy-dylibs` (a **default** feature) copies `.dll`s into the output dir. Maintainer,
  on a Tauri app doing exactly this with CPU-only: "the binaries provided are compiled with DirectML
  by default, and since they are statically linked, the final executable must 1) be linked to
  DirectML… and 2) be able to load DirectML at runtime"
  ([ort#470](https://github.com/pykeio/ort/issues/470)). **But DirectML "is distributed as a system
  component of Windows… in Windows 10, version 1903 (Build 18362) and newer."** Ship the copied DLL in
  the app folder (the installer does this automatically) or rely on the inbox one — **no user install
  step either way.**
- **VERIFIED — the Windows build needs network on first build**, same as macOS: `fetch_file()` GETs
  the parcel.pyke.io URL and asserts the SHA256.
  **Cache: `%LOCALAPPDATA%\ort.pyke.io\dfbin\<target-triple>\<sha256>\onnxruntime\`**
  (`ort-sys/src/internal/dirs.rs` → `SHGetKnownFolderPath(FOLDERID_LOCAL_APP_DATA)` joined with
  `ort.pyke.io`). **The download is skipped if `lib_dir` already exists**, so pre-seeding that path
  gives an offline build.
- **VERIFIED — `ORT_LIB_LOCATION`** points `ort` at your own ONNX Runtime build; if
  `<dir>/onnxruntime.lib` exists it emits `static=onnxruntime` directly, otherwise it probes
  `Release/RelWithDebInfo/MinSizeRel/Debug` (override via `ORT_LIB_PROFILE`). Setting it **bypasses
  the download entirely**. `ORT_PREFER_DYNAMIC_LINK=1` forces dynamic. All are `rerun-if-env-changed`.

**The MSVC runtime — the sharpest risk in the whole port**

- **VERIFIED — Rust does NOT statically link the CRT by default on `x86_64-pc-windows-msvc`.**
  `crt_static_default` is left at the `TargetOptions` default of `false`. ChrisDenton (Rust Windows
  maintainer): "**By default, Rust requires programs to deploy `vcruntime140.dll` (or equivalent) when
  redistributing binaries.**" The Universal CRT itself "is a component of Windows so can always be
  dynamically linked."
- **VERIFIED — Tauri already solves this by default.** `build.windows.staticVCRuntime` **defaults to
  `true`** (`config.rs:3593-3605`), and `tauri-build` then calls a vendored copy of
  `static_vcruntime` ("we're not using static_vcruntime directly because we want this for debug builds
  too") emitting `/NODEFAULTLIB:msvcrt.lib …` plus `/DEFAULTLIB:libcmt.lib libvcruntime.lib ucrt.lib`,
  and stubbing the hard-coded `msvcrt.lib` with an empty object. Result: **static VC runtime + static
  C startup + dynamic UCRT (inbox on Win10+). No VC++ redist needed for the Tauri exe.**
- **VERIFIED — but `ort`'s prebuilt `onnxruntime.lib` is built `/MD` (dynamic CRT).** The linker error
  in the wild names it exactly: `LNK2038: mismatch detected for 'RuntimeLibrary': value
  'MT_StaticRelease' doesn't match value 'MD_DynamicRelease' in
  libort_sys-….rlib(onnxruntime_c_api.obj)`. Maintainer: "ONNX Runtime links CRT dynamically by
  default." ([ort#329](https://github.com/pykeio/ort/issues/329))
- **VERIFIED — `-C target-feature=+crt-static` outright fails with `ort`.**
  [ort#473](https://github.com/pykeio/ort/issues/473) "Can't build on windows when static crt is
  used" → "A bunch of missing symbols (from crt)". The maintainer's only remedy is compiling ONNX
  Runtime from source with `--enable_msvc_static_runtime`. **Do not reach for `+crt-static`.**
- **UNVERIFIED and load-bearing — whether Tauri's default `staticVCRuntime: true` collides with
  `ort`.** Tauri's mechanism is *not* `+crt-static`; it is `/NODEFAULTLIB` plus an empty `msvcrt.lib`
  stub, a weaker intervention, and Rust objects do not emit the `/FAILIFMISMATCH` `RuntimeLibrary`
  directive that triggers LNK2038 (in #329 that came from `esaxx-rs`, a C++ crate). ort#470 shows a
  Tauri + `ort` app that evidently built. **Predict a link failure; plan for it; test it first
  (question 32).**
- **VERIFIED — the fallback is clean and already supported**: `build.windows.staticVCRuntime = false`
  + `bundle.windows.bundleVCRuntime = true`. Tauri's docs for that flag: "This can be particularly
  useful when your application includes **sidecars or DLLs that do not statically link the Visual C++
  runtime**… and you do not want to require users to install the Visual C++ Redistributable." The
  bundler locates the redist via a bundled `vswhere.exe` and copies `Microsoft.VC*.CRT\*.dll` into the
  app.
- **VERIFIED — and that fallback still satisfies constraint 1**, because Microsoft sanctions it: "**In
  local deployment, library files are installed in your application folder together with the
  executable file.**" (As opposed to central deployment via `vc_redist.x64.exe`.) Watch the
  dot-library caveat — you may need `msvcp140_1.dll` alongside `msvcp140.dll`.
- **UNVERIFIED — Aloud has a sidecar helper binary**, so `bundleVCRuntime: true` may be needed
  regardless of how the main exe links. Depends on how the helper is built.

**GPU execution providers — opt-in twice, and correctly so**

- **VERIFIED — CPU is the default.** EPs require both their Cargo feature and explicit registration
  via `SessionBuilder::with_execution_providers`; "If an EP does not support a certain operator… it
  will fall back to the next successfully registered EP, or **to the CPU if all else fails**." No EP
  feature is in `ort`'s default set.
- **VERIFIED — DirectML EP has static binaries available**, so it could use the RTX 2070 Super with no
  new runtime dependency beyond the inbox `DirectML.dll`. **CUDA/TensorRT are dynamic-only** — forcing
  DLL shipping *and* a CUDA/cuDNN install on the user's machine, which is a hard constraint conflict.
  `dist.txt` maps `cu12` to `ortrs_dylib_cu12-…` and `prefer_dynamic_linking()` returns `true`
  unconditionally for those features. **Do not enable them.**

### 5.6 Toolchain the PC needs — verbatim for the brief

- **VERIFIED — Rust**: `rustup default stable-msvc`; host triple **`x86_64-pc-windows-msvc`**. Tauri:
  "For full support for Tauri and tools like trunk make sure the MSVC Rust toolchain is the selected
  default host triple."
- **VERIFIED — MSVC, not GNU.** `ort` ships no `x86_64-pc-windows-gnu` prebuilt (`dist.txt` has only
  `-msvc` rows for Windows; a MinGW request, ort#561, was closed). Independently, §1.6 found that
  `tauri-build` drops a `WebView2Loader.dll` beside the exe on the GNU toolchain. Two reasons, same
  answer.
- **VERIFIED — Visual Studio Build Tools with the "Desktop development with C++" workload** (brings
  the Windows SDK, needed for `dxguid.lib` / `DXGI.lib` / `D3D12.lib` / `DirectML.lib`).
- **VERIFIED — `ort` demands a newer VS than Tauri does**: "A recent version of **Windows 10/11 &
  Visual Studio 2022 (≥ 17.10)** are required for pyke binaries."
- **VERIFIED — for MSI only**: enable the **VBSCRIPT** optional Windows feature.
- **VERIFIED — `bundleVCRuntime: true` additionally requires** a VS install carrying
  `Microsoft.VisualStudio.Component.VC.Redist.14.Latest`, or `VCTOOLS_REDIST_DIR` set.

### What this means for M6

1. **Target NSIS with `"installMode": "currentUser"`.** It is the only Tauri-native target that
   installs without admin, keeps `%APPDATA%` unvirtualized, allows the HKCU Run key that
   `tauri-plugin-autostart` needs, and carries the sidecar — with no MSIX-style trust step. MSI forces
   per-machine + elevation for no benefit here.
2. **Rule MSIX out for the primary channel.** Tauri cannot build it, sideloading needs a *trusted*
   cert (an admin "first install X" step — constraint conflict), the `%APPDATA%` redirection question
   is unresolved in Microsoft's own docs, and the Run-key startup entry is likely virtualized away.
   It is rational only as a *second* channel via the Microsoft Store, where Microsoft re-signs free.
3. **Do not buy a certificate for M6.** Windows has no TCC analogue; signing is purely a distribution
   concern, and — critically — **EV no longer bypasses SmartScreen**, so the usual "just buy EV"
   advice is obsolete. Revisit only when Aloud is actually distributed.
4. **When that day comes, the shortlist is:** SignPath Foundation (free, if Aloud goes open source),
   or an OV certificate with cloud-HSM signing (~€139–300/yr, individuals eligible, ~15-month renewals
   under the new 460-day rule). **Azure Artifact Signing is unavailable** — individual developers must
   be in the US or Canada.
5. **Build MSVC. Never GNU, never `+crt-static`.** Then **test the CRT interaction first** (question
   32) — it is the most likely thing to stop the PC dead, and the fallback
   (`staticVCRuntime: false` + `bundleVCRuntime: true`) is a two-line config change that still honours
   constraint 1 via Microsoft-sanctioned local deployment.
6. **Copy the ONNX build-time facts into the brief verbatim**: first build needs network; cache lives
   at `%LOCALAPPDATA%\ort.pyke.io\dfbin\…`; `ORT_LIB_LOCATION` bypasses the download; **NSIS/WiX and
   WebView2 are two further first-build downloads**, so budget the PC's first `tauri build` as an
   online operation in three independent ways.
7. **Ship `DirectML.dll` beside the exe and say nothing more about it** — it is inbox on Windows 10
   1903+, the installer copies it automatically, and it is not a user-facing dependency. **Never
   enable the `cuda` or `tensorrt` features** — they are dynamic-only and would require the user to
   install CUDA.

---

## 6. Anything else Windows silently requires

The macOS analogue is `NSRequiredContext`: a manifest key whose absence causes wrong behaviour with
no error anywhere. These are the Windows shapes of that failure.

### 6.1 The application manifest — Tauri's default contains almost nothing

- **VERIFIED — Tauri 2's built-in Windows manifest declares one thing: Common-Controls v6.** The
  whole file is a single `<dependency>` on `Microsoft.Windows.Common-Controls` 6.0.0.0. There is
  **no `<dpiAware>`, no `<dpiAwareness>`, no `<longPathAware>`, no `<activeCodePage>`, no
  `<trustInfo>`/`requestedExecutionLevel`.**
  (`gh api repos/tauri-apps/tauri/contents/crates/tauri-build/src/windows-app-manifest.xml`)
- **VERIFIED — it is replaceable in one line.** `WindowsAttributes::app_manifest(...)` overwrites it
  wholesale, and `tauri-build` passes it to `WindowsResource::set_manifest`
  (`crates/tauri-build/src/lib.rs:670`). Note *wholesale*: a custom manifest must re-declare the
  Common-Controls dependency or Tauri's dialog APIs lose their v6 theming (documented in the
  attribute's own doc comment).

### 6.2 DPI awareness — set at runtime by tao, not by the manifest

- **VERIFIED — tao sets per-monitor-v2 awareness programmatically at `EventLoop` creation.**
  `if attributes.dpi_aware { become_dpi_aware(); }` where `dpi_aware` defaults to `true`
  (`tao/src/platform_impl/windows/event_loop.rs:153-187`), and `become_dpi_aware()` calls
  `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`, falling back to
  `PER_MONITOR_AWARE`, then `SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)`, then
  `SetProcessDPIAware()` (`tao/src/platform_impl/windows/dpi.rs:20-40`).
- **VERIFIED — Microsoft explicitly prefers the manifest and warns about the ordering.** "We
  recommended that you specify the default process DPI awareness via a manifest setting. While
  specifying the default via API is supported, it is not recommended… Once a window (an HWND) has
  been created in your process, changing the DPI awareness mode is no longer supported… you must
  call the corresponding API before any HWNDs have been created."
  ([Setting the default DPI awareness for a process](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process))
- **VERIFIED — with no manifest setting and no API call, a process is DPI *unaware* by default**
  ("Absent — The current process is dpi unaware by default",
  [Application manifests, `dpiAware`](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests)).
  A DPI-unaware process is virtualised: it sees a scaled, lied-to coordinate space, which would
  silently corrupt every capture rectangle on a scaled monitor.
- **The actual risk for Aloud is the ordering rule, and it is a live one.** Aloud's region path is a
  *native* code path (capture + OCR helper), and anything that creates an `HWND` **before**
  `EventLoop::new()` — an overlay window created early, a helper, a dependency — permanently locks
  the process into unaware mode, with no error. The symptom would be wrong capture coordinates on a
  scaled or multi-monitor setup, i.e. exactly the failure mode hard constraint 4 exists to prevent.
- **Recommendation: declare it in the manifest anyway.** Adding
  `<dpiAwareness>PerMonitorV2, PerMonitor</dpiAwareness>` + `<dpiAware>true/pm</dpiAware>` via
  `WindowsAttributes::app_manifest` makes the mode true from process start regardless of HWND
  ordering, matches Microsoft's recommendation, and costs nothing. tao's runtime call then becomes a
  redundant no-op rather than the only line of defence.

### 6.3 Other manifest settings worth a deliberate decision

All from [Application manifests](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests) — `VERIFIED`:

- **`<longPathAware>true</longPathAware>`** (Windows 10 1607+) — "Enables long paths that exceed
  `MAX_PATH` in length." Aloud writes a 385 MB model under a user-profile path and reads image files
  from arbitrary locations. Without this, a path over 260 chars fails as an ordinary I/O error with
  no hint that the length was the cause. Note it *also* requires the machine-wide
  `LongPathsEnabled` policy; the manifest is the app's half.
- **`<activeCodePage>UTF-8</activeCodePage>`** (Windows 10 1903+) — forces the process ANSI code page
  to UTF-8. Aloud's whole job is multilingual text (en/de/uk/ru) crossing a Rust ↔ Win32 ↔ WinRT
  boundary. Without it, any accidental `*A` API call mangles non-Latin-1 text silently. Cheap
  insurance; note the doc's caveat that on Windows 11 this element gained `Legacy`/locale values.
- **`<trustInfo>` / `requestedExecutionLevel level="asInvoker"`** — "All UAC-compliant apps should
  have a requested execution level added to the application manifest." Also: "Specifying
  `requestedExecutionLevel` node will disable file and registry virtualization." Tauri's default
  manifest omits it. `asInvoker` is what Aloud wants (elevation would break global hotkeys' UIPI
  situation further — see §3). MSVC's linker embeds an `asInvoker` UAC fragment by default, but
  Tauri's `set_manifest` replaces the manifest, so confirm the level actually present in the built
  exe (PC question).
- **`<supportedOS>`** — "Application manifests without a compatibility element default to Windows
  Vista compatibility on Windows 7." Tauri declares none. Declaring the Windows 10/11 GUID
  `{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}` is standard hygiene and affects some compatibility
  shims.
- **`<heapType>SegmentHeap</heapType>`** (Windows 10 2004+) — optional; would reduce memory
  footprint for a resident app holding a 385 MB model. Not required; note and move on.

### 6.4 The tray icon can fail to appear at login, silently

- **VERIFIED — `tray-icon` swallows a failed `Shell_NotifyIcon(NIM_ADD)` at construction.** The code
  is literally:
  ```rust
  if !register_tray_icon(hwnd, internal_id, &hicon, &attrs.tooltip, true) {
      // Explorer/taskbar may not be ready yet (e.g., app starts before explorer.exe).
      // Keep the window alive and wait for TaskbarCreated to re-register.
  }
  ```
  (`tray-icon/src/platform_impl/windows/mod.rs:153-157`) — an empty branch. No error, no log.
- **VERIFIED — recovery is wired**: it registers the `"TaskbarCreated"` broadcast message and
  re-adds the icon when it fires (`mod.rs:50-53, 363-372`), and it calls
  `ChangeWindowMessageFilterEx(hwnd, S_U_TASKBAR_RESTART, MSGFLT_ALLOW, ...)` so the message crosses
  UIPI (`mod.rs:148-149`).
- **Why this matters for M6 specifically:** launch-at-login is exactly the scenario where the shell
  may not be ready. If the icon never appears at login, the failure is invisible from inside Rust —
  Aloud must log its own `Shell_NotifyIcon` outcome, or it will be undebuggable in the same way the
  macOS build was before `~/Library/Logs/Aloud/aloud.log` existed. **Port the logging file first, as
  on macOS. It is the single highest-leverage debugging investment.**

### 6.5 Missing-icon fallback

- **VERIFIED — there is no silent fallback for the app icon.** `tauri-build` errors out if the `.ico`
  is absent (§1.3). This is the good case: loud.
- **UNVERIFIED — behaviour when the tray RGBA is present but malformed.** `Icon::from_rgba` returns
  `BadIcon`, Tauri's `TrayIconBuilder::icon` does `icon.try_into().ok()` and **drops the error**
  (`crates/tauri/src/tray/mod.rs:252-258`) — an invalid icon silently becomes *no icon*, i.e. an
  invisible tray entry. Guard this in Aloud's own code by validating the image before handing it over.

### What this means for M6

1. **Ship a custom Windows application manifest via `WindowsAttributes::app_manifest`**, containing:
   the Common-Controls v6 dependency (must be re-declared — the override is wholesale),
   `dpiAwareness = PerMonitorV2, PerMonitor` + `dpiAware = true/pm`, `longPathAware = true`,
   `activeCodePage = UTF-8`, `requestedExecutionLevel level="asInvoker" uiAccess="false"`, and the
   Windows 10/11 `supportedOS` GUID. This is one file and closes four silent-failure classes.
2. **Port `aloud.log` to Windows before anything else in M6** — `%LOCALAPPDATA%\Aloud\logs\aloud.log`
   or equivalent. Every macOS diagnosis came from that file, and the Windows failure modes above are
   *more* silent, not less.
3. **Log the `Shell_NotifyIcon(NIM_ADD)` result explicitly**, since `tray-icon` will not.
4. **Validate the tray image before passing it to Tauri** — `TrayIconBuilder::icon` discards the
   error and you get an invisible tray icon.
5. **Do not create any `HWND` before `EventLoop::new()`** — it permanently locks the process to
   DPI-unaware. The manifest in (1) makes this safe, but keep the ordering rule anyway.

---

## Constraint conflicts

Every place Windows collides with an Aloud hard constraint. **These are owner decisions — options and
a recommendation are given, nothing is decided here.**

### C1. WebView2 vs "zero runtime system dependencies" (constraint 1)

**The conflict.** Tauri renders its UI in WebView2. Microsoft states the Evergreen Runtime "will be
included as part of the Windows 11 operating system" and that "the vast majority of Windows 10
devices have the WebView2 Runtime installed already" — but explicitly not all, and recommends apps
check and install it. Tauri's own docs claim it ships with Windows 10 April-2018+, which is stronger
than Microsoft's own statement. So on Windows, "everything bundled or statically linked" is not
automatically true.

**Options.** (a) `downloadBootstrapper` — 0 MB, needs internet at install, the default.
(b) `embedBootstrapper` — +1.8 MB, still needs internet for the runtime.
(c) `offlineInstaller` — +127 MB, fully offline, still Evergreen (shared, auto-updating).
(d) `fixedVersion` — +180 MB installer / >250 MB on disk, fully offline and pinned, **but** on
Windows 10 an unpackaged Win32 app must additionally run two `icacls` grants for Fixed Version ≥120
to work at all, and it cannot run from a UNC path. (e) `skip` — violates the constraint outright.

**Recommendation.** For Andrii's own PC, (a) is fine and costs nothing — the constraint's *purpose*
("no README of setup chores") is satisfied because the runtime is already there. The moment Aloud is
distributed, (c) is the honest reading of the constraint: fully offline, no user step, and it avoids
(d)'s Windows 10 App Container trap. State the choice explicitly in `tauri.conf.json` rather than
inheriting the default silently.

### C2. Windows OCR language packs vs "zero runtime system dependencies" (constraint 1)

**The conflict, and it is the sharpest one.** `Windows.Media.Ocr`'s *engine* is inbox on every SKU,
but its *language models* are Features-on-Demand. Microsoft's own instructions are: administrator
PowerShell, `Add-WindowsCapability -Online`, "may take several minutes" (or Settings on Windows 11 for
a standard user). **That is exactly the "first install X" step constraint 1 exists to forbid**, and
`TryCreateFromLanguage` fails by returning `null` — silently. A stock en-US machine has English OCR
and nothing else; German and Russian only if the user added those language features.

**Options.** (a) Constrain the feature to what the machine has: enumerate
`AvailableRecognizerLanguages` at startup, drive the UI from it, default via
`TryCreateFromUserProfileLanguages()`. Zero dependencies preserved, coverage degraded.
(b) Detect and instruct: (a) plus a one-time deep link to Settings — honest, but it *is* the banned
first-install step. (c) Bundle an OCR model (ONNX/RapidOCR-class) — preserves zero-deps, unlocks
Ukrainian, costs binary size, and requires re-reading the "never Tesseract" rule (which bans Tesseract
specifically, not all bundled OCR). (d) Programmatic silent install — likely a dead end: needs admin
and internet, and a tray utility should not request elevation.

**Recommendation.** (a) as the floor — it is the only option that keeps the constraint fully intact,
and it is required regardless of what else is chosen, because the UI must never offer a language the
engine cannot do. Whether to add (b) or invest in (c) is a product call that depends on C3.

### C3. Ukrainian OCR does not exist on Windows — a feature-parity conflict

**The conflict.** `lingua-rs` is configured for English, German, Ukrainian and Russian, and Andrii's
own context is Ukrainian. **LIKELY** (Microsoft Q&A, not an API reference) that `Windows.Media.Ocr`
has no Ukrainian recognizer at any price. Because lingua runs *after* OCR, it cannot recover glyphs
the recognizer never produced — Ukrainian read through the Russian model will mangle і, ї, є, ґ before
detection ever runs.

**Options.** (a) Accept three-language OCR on Windows; keep four on macOS (Vision does support
Ukrainian). Cross-platform feature asymmetry, documented. (b) Bundle an OCR engine on Windows only —
resolves C2 and C3 together, at the cost of a second `OcrEngine` implementation behind the existing
seam. (c) Bundle one engine on *both* platforms and drop the Vision helper — most uniform, largest
rewrite, discards working macOS code.

**Recommendation.** Confirm the premise first (question 16) — it is LIKELY, not VERIFIED, and the
whole branch depends on it. If confirmed, (a) is the cheap correct answer for a personal tool and (b)
is the answer if Ukrainian screen-reading is actually a use Andrii has. This is a *product* question,
not a technical one.

### C4. The WGC yellow border vs the crosshair-drag UX (constraint 4's neighbourhood)

**The conflict.** `Windows.Graphics.Capture` draws a system yellow border around the captured item,
and `IsBorderRequired = false` requires the `graphicsCaptureWithoutBorder` capability **in a package
manifest** — which a non-packaged app does not have. The property will appear to succeed and be
ignored. Meanwhile packaging as MSIX to remove the border drags in package identity, filesystem
virtualization, and signing.

**Options.** (a) Use GDI `BitBlt` instead — no border, no packaging pressure, one call, natively spans
the virtual desktop including negative coordinates. Loses HDR correctness and is slower per pixel.
(b) Use WGC and accept a yellow flash on every capture. (c) Go MSIX purely to disable the border.

**Recommendation.** (a). Aloud captures one still frame, not a video stream; that is BitBlt's exact
shape, and both alternatives pay real costs for capabilities Aloud does not use. Note that all three
capture APIs return black over `WDA_EXCLUDEFROMCAPTURE` windows — that is not a differentiator.

### C5. Elevating for global hotkeys vs launch-at-login — mutually exclusive

**The conflict.** If the UIPI claim in §3.3 turns out to be real for `RegisterHotKey`, the only fix is
running Aloud elevated. But Windows **blocks** `requireAdministrator` apps launched from the Run key
or Startup folder (VERIFIED). Elevating to fix hotkeys therefore breaks launch-at-login outright, and
restoring it needs a Task Scheduler `TASK_RUNLEVEL_HIGHEST` task that can only be registered from an
already-elevated process. The `uiAccess="true"` middle path requires a signed binary in a secure
location, which an unsigned per-user install does not have.

**Options.** (a) Stay non-elevated and document the limitation (the project's current position).
(b) Elevate and rebuild autostart on Task Scheduler.

**Recommendation.** (a), and **test the premise before documenting anything** (question 24). The
evidence that `RegisterHotKey` is affected at all is weak — the strong evidence is about
`SetWindowsHookEx`, a different mechanism. Do not ship a documented limitation nobody has observed.

### C6. MSIX sideloading vs "zero runtime system dependencies" (constraint 1)

**The conflict.** An untrusted or self-signed MSIX cannot install until the user manually imports a
certificate into Trusted People — an admin-elevated "first install X" step, which is exactly what
constraint 1 forbids. The cause is the certificate, not the package format.

**Options.** (a) Don't use MSIX — NSIS `currentUser` has none of this. (b) MSIX via the Microsoft
Store, where Microsoft re-signs with its own certificate for free. (c) MSIX sideload with a purchased
CA-trusted cert.

**Recommendation.** (a) for the primary channel — reinforced by four independent facts: Tauri cannot
build MSIX at all, the `%APPDATA%` virtualization question is unresolved in Microsoft's own docs, the
HKCU Run key that autostart depends on is likely virtualized away, and Azure Artifact Signing is
closed to a German individual. (b) stays available later as a *second* channel if Aloud is ever sold.

### C7. The MSVC runtime vs "zero runtime system dependencies" (constraint 1)

**The conflict, and it is the most likely thing to stop the PC dead.** Rust does not statically link
the CRT on `x86_64-pc-windows-msvc` by default, so a naive build needs `vcruntime140.dll` on the
user's machine. Tauri already fixes that (`staticVCRuntime` defaults to `true`) — but `ort`'s prebuilt
`onnxruntime.lib` is compiled `/MD` (dynamic CRT), and `-C target-feature=+crt-static` is **VERIFIED**
to fail outright with `ort`. Whether Tauri's weaker `/NODEFAULTLIB` approach also collides is
**UNVERIFIED**.

**Options.** (a) Keep `staticVCRuntime: true` and hope it links (test first).
(b) `staticVCRuntime: false` + `bundleVCRuntime: true` — Tauri copies the CRT DLLs into the app
folder. (c) Compile ONNX Runtime from source with `--enable_msvc_static_runtime` and point
`ORT_LIB_LOCATION` at it.

**Recommendation.** (a) if it links, else (b). **(b) still satisfies constraint 1** — Microsoft
explicitly sanctions it: "In local deployment, library files are installed in your application folder
together with the executable file." There is no user install step either way. (c) is a large amount of
work for a marginal gain and should not be attempted before (a) and (b) are ruled out. Note this is a
*two-line config change*, not a redesign — the conflict is loud, not silent.

**Non-conflicts, recorded so nobody re-opens them.** `DirectML.dll` (copied beside the exe by `ort`)
is **VERIFIED** inbox on Windows 10 1903+, so it is not a user dependency. ONNX Runtime itself links
**statically** on Windows exactly as on macOS — there is no `onnxruntime.dll` to ship. Both were open
questions before this pass.

---

## Must be verified on the PC

Numbered, self-contained, and written so a person with no context can answer each on a Windows
machine. Paste verbatim into a `BKM/PC-Queue/` brief.

**Tray, window and WebView2**

1. Install the app, then find its icon in the taskbar's hidden-icons flyout (the `^` chevron) and drag
   it out so it is always visible. Now quit the app and start it again. **Is the icon still visible in
   the taskbar, or has it gone back into the hidden flyout?**
2. Repeat question 1, but this time rebuild the app from source (producing a new .exe at the same
   path) before restarting it. **Is the icon still visible in the taskbar, or back in the flyout?**
3. Uninstall the Microsoft Edge WebView2 Runtime (Settings → Apps → Installed apps). Then launch the
   app. **Does the tray icon appear at all?** Then click the tray menu item that opens the settings
   window. **Does a window open, does an error message appear, or does nothing happen?** Copy any
   error text. Reinstall WebView2 afterwards.
4. Check whether WebView2 is installed by opening `regedit` and looking at
   `HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`
   and the same path under `HKEY_CURRENT_USER\Software\...`. **Which of the two exists, and what is
   the `pv` value?**
5. Open the settings window from the tray menu while a different application (e.g. Notepad) is in the
   foreground. **Does the settings window come to the front and receive keyboard focus, or does it
   open behind the other window / flash in the taskbar without focusing?**
6. Set the display scale to 100%, then 150%, then 200% (Settings → System → Display → Scale). At each
   setting, look closely at the app's tray icon. **At which scale settings does it look sharp, and at
   which does it look blurry or fuzzy?**

**Launch at login**

7. Turn on "launch at login" in the app. Open `regedit` →
   `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`. **Is the value's data wrapped in
   quotation marks, and does the path it contains include any spaces?** Copy the exact string.
8. With that Run value present, open Task Manager → Startup apps and set the app to **Disabled**. Then
   look at
   `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`.
   **Is there a value with the app's name, how many bytes is it, and what are the first four bytes in
   hex?** Repeat after setting it back to **Enabled** and report both byte sequences.
9. With the app disabled in Task Manager (question 8), open the app's own settings and read what it
   reports for "launch at login". **Does the app say enabled or disabled?**
10. Turn "launch at login" **off** in the app, then click it **off a second time**. **Does the second
    click produce an error message or fail silently?**
11. Reboot with "launch at login" on. **How many seconds after the desktop appears does the app's tray
    icon show up?** Time it with a phone stopwatch, then reboot again and time it a second time.
12. Turn "launch at login" on, reboot, let the app run, shut down, and reboot again. **After the second
    reboot, is the Run value from question 7 still present in the registry?**
13. Install an update of the app over the top of an existing install. **Is the Run value from question
    7 still present afterwards, with the same data?**
14. Create a scheduled task in Task Scheduler that runs a program at logon for the current user,
    without ticking "Run with highest privileges". **Does creating it require a UAC prompt?** Then open
    Task Manager → Startup apps. **Does the scheduled task appear in that list?**
15. Download the unsigned installer through Microsoft Edge and run it. **Does a blue "Windows protected
    your PC" box appear, and does it offer a "Run anyway" link or only an OK button?** Then check
    Windows Security → Protection history. **Is there any entry mentioning the app?**

**OCR and capture**

16. Open **Windows PowerShell as Administrator** (not PowerShell 7) and run
    `Get-WindowsCapability -Online | Where-Object { $_.Name -Like 'Language.OCR*' }`. Paste the full
    output. **Is there a row whose name contains `uk-UA`?** Which rows say `State : Installed` and
    which say `NotPresent`?
17. In Windows PowerShell run
    `[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]` then
    `[Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages`. **Which languages come back on this
    machine as it is configured today?**
18. Run `winver`, and `Get-ComputerInfo | Select WindowsProductName, OsBuildNumber`. **What are the
    exact Windows edition and build number?**
19. Build and run the sample program `robmikh/screenshot-rs` with `cargo run -- --monitor 1 out.png`.
    **Does it produce a correct PNG with no permission prompt, no consent dialog and no elevation? Does
    a yellow border appear on screen while it runs?**
20. In that same run, compare the PNG's pixel dimensions to the monitor's resolution, **on a monitor
    set to a display scale other than 100%. Are they the same number, or is the PNG smaller?**
21. Capture the entire virtual desktop on a multi-monitor setup with **different** scale factors, where
    one monitor is positioned to the left of or above the primary one (so coordinates go negative).
    **Does the captured image line up 1:1 with what is on screen on both monitors, or is one of them
    offset, stretched, or cropped?**
22. Find the smallest region, in pixels, that still returns recognized text from Windows OCR rather
    than an empty result. **What are its width and height?** Also report the value of
    `OcrEngine.MaxImageDimension` on this machine.
23. Open a video in the Netflix app or a banking site in Edge, then capture that region with the app.
    **Is the captured area black?**

**Global shortcuts**

24. **(Highest priority — this decides whether a limitation currently written into the project's docs
    is real.)** With the app running normally (NOT as administrator), open an elevated window: press
    Start, type `cmd`, right-click Command Prompt, choose **Run as administrator**. Click inside that
    black window so it has focus. **Now press the app's global shortcut. Does the app respond?** Then
    click on a normal window such as Notepad and press it again to confirm the shortcut works at all.
    **Report both results.**
25. Start a second program and try to register the **same** key combination the app already uses.
    **Does the second registration produce a visible error, and does the error text contain the words
    "already registered"?**
26. In the app's settings, open the shortcut-capture field and press each of these in turn: `Win+L`,
    `Ctrl+Alt+Del`, `Alt+Tab`, `Win+R`, `F12`, `PrintScreen`. **For each one, does the capture field
    show anything at all, or does nothing happen?** List which registered and which were ignored.
27. Switch the keyboard layout to **German (QWERTZ)** (Settings → Time & language → Language & region).
    In the shortcut-capture field, press `Ctrl+Alt+` the key physically labelled **Y** on a German
    keyboard. **What chord does the app display?** Then switch back to the US layout, restart the app,
    and check whether the saved shortcut still fires when you press that same physical key.
28. With a German layout active, try to capture a chord using a dead key (`´`, `` ` `` or `^` — the keys
    right of `P` and `Ö`). **Does the capture field accept it, show nothing, or show something odd?**
29. Assign the app a shortcut, close the app, reopen it, assign a **different** shortcut. **Does the old
    shortcut stop working, and does the new one work?**

**Paths**

30. Find the app's settings file on disk. Check both `C:\Users\<you>\AppData\Roaming\` and
    `C:\Users\<you>\AppData\Local\`. **Which of those two folders contains the app's config, and what
    exactly is the folder called?** Then find where the downloaded voice/model files landed and report
    that full path too.
31. Uninstall the app **without** ticking any "delete app data" checkbox. **Are the folders from
    question 30 still on disk afterwards?** Check each path separately and say which survived.

**Build and toolchain — do these FIRST, they gate everything else**

32. **(Highest priority — most likely to stop the build dead.)** With the Rust MSVC toolchain
    installed, build the project for `x86_64-pc-windows-msvc` leaving Tauri's default
    `build.windows.staticVCRuntime = true` in place. **Does it link successfully, or does it fail with
    an error containing `LNK2038` and the words "mismatch detected for 'RuntimeLibrary'"?** Paste the
    full error if it fails.
33. If question 32 failed: set `"staticVCRuntime": false` under `build.windows` and
    `"bundleVCRuntime": true` under `bundle.windows` in `tauri.conf.json`, then rebuild. **Does it
    link now, and which extra `.dll` files appear next to the built `.exe`?**
34. Run `dumpbin /dependents` on the built `.exe`. **Which DLLs does it import?** Specifically say
    whether `vcruntime140.dll`, `msvcp140.dll`, `DirectML.dll` or `onnxruntime.dll` appear in the list.
35. List every file sitting next to the `.exe` in `target\release\`. **Is `DirectML.dll` there, and
    what is its size in MB?**
36. Copy the built `.exe` plus every file beside it onto a Windows machine that has **never** had
    Visual Studio or the Visual C++ Redistributable installed. **Does it launch, or does it show an
    error about a missing DLL?** Copy the exact error text.
37. After a successful build, list what exists under
    `%LOCALAPPDATA%\ort.pyke.io\dfbin\x86_64-pc-windows-msvc\` and under `%LOCALAPPDATA%\tauri\NSIS`.
    **What are the folder names and total sizes?**
38. Disconnect the machine from the network entirely. Run the packaging build a second time.
    **Does it succeed using only the caches from question 37, or does it fail trying to download
    something?** Name what it tried to download.

**Packaging and signing**

39. Build the NSIS installer with `"installMode": "currentUser"`. Install it from a
    **non-administrator** account. **Does it complete without any UAC prompt, and what folder does it
    install into?**
40. After that install, confirm the app can write to `%APPDATA%` and download a large (>300 MB) file
    to a user-writable folder. **Does either action produce a prompt or an error?**
41. Download the unsigned installer using Microsoft Edge and run it. **What is the exact dialog text,
    and is there a "More info" → "Run anyway" path, or is it blocked outright with no way to proceed?**
42. Open Windows Security → App & browser control. **Is "Smart App Control" present on this machine,
    and is it set to On, Evaluation, or Off?**
43. Over a week of normal development, **does Microsoft Defender ever quarantine or delete a freshly
    built unsigned binary from `target\release\`?** If yes, copy the entry from Windows Security →
    Protection history.
44. Launch the app and let it bind any network socket. **Does a "Windows Security Alert" firewall
    dialog appear? Then rebuild the app to the same output path and launch it again — does the dialog
    reappear, or is the previous rule still in effect?**
45. On a Windows 10 machine that does **not** have the WebView2 Runtime installed, run the NSIS
    installer with its default settings. **Does it install WebView2 successfully, and is the process
    visible to the user or silent?**
