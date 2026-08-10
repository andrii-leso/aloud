# ⌘⇧A takes focus away from the window you were working in

**Reported:** 2026-08-10, from real use — pressing the selection shortcut while working
in another app moves focus off that window before Aloud starts speaking.
**Status:** reproduced on the current build, mechanism identified, fixed.
**Machine:** M1 MacBook Air, macOS 26.6 (25G72), `/Applications/Aloud.app` self-signed
"Aloud Dev".

---

## 1. Does it still reproduce? Yes.

The leading hypothesis going in was that the report predated commit `539bda4` ("closing
the settings window no longer quits the app"). While that bug existed, every ⌘⇧A after a
settings-window close *relaunched* Aloud through LaunchServices, and a launch runs tao's
unconditional `activateIgnoringOtherApps`
(`tao-0.35.3/src/platform_impl/macos/app_delegate.rs:107`) — which would steal focus for
a completely different reason.

**That did happen, and it is not the whole story.** The log records exactly one instance
of the relaunch signature, and it is the one the fixed bug produced:

```
16:19:09 selection: Service callback fired, pasteboard text length=387 chars
16:19:21 probe: disarmed (settings window closed)        <- the settings close killed the app
16:20:26 app start                                        <- ⌘⇧A relaunched it
16:20:28 selection: Service callback fired, pasteboard text length=205 chars
```

But the same log also shows the Service firing 3.2 hours into a single process
(`16:15:10`, `17:17:04`, `17:19:15` — no `app start` anywhere near them), so the app
being already running is the normal case, and the question of what happens *then* was
still open.

**Answered by direct measurement.** `NSPerformService("Read Aloud", pasteboard)` invokes
the Service programmatically exactly as the keyboard does, and `NSWorkspace`'s
`frontmostApplication` was sampled every 100 ms across it. With the app already running
(same pid before and after, no `app start` in the log):

```
17:45:44.404 BEFORE  frontmost = Finder [com.apple.finder] pid=1298
17:45:44.719 NSPerformService("Read Aloud") -> true
17:45:44.823 CHANGE  frontmost = Aloud [com.andriileso.aloud] pid=30089
17:45:59.816 AFTER   frontmost = Aloud [com.andriileso.aloud] pid=30089
```

Focus moved to Aloud ~110 ms after the service call and stayed there. The bug is real,
it is independent of the relaunch bug, and it survives on the current build.

---

## 2. Mechanism

**macOS activates the Services provider as part of delivering the service message.** It
is already done before any Aloud code runs.

Instrumented build, first statement of `-readSelection:userData:error:`, reading
`NSRunningApplication.currentApplication.isActive`:

```
DIAG: handler entry, isActive=true
selection: Service callback fired, pasteboard text length=10 chars
DIAG: +30ms isActive=true
DIAG: +100ms isActive=true   ... +300ms, +800ms, +2000ms all true
```

So there is nothing to decline. Aloud is not *asking* to be activated:

- Nothing on the selection path calls `activate`, `activateIgnoringOtherApps`,
  `NSApp.activate`, `orderFront`, or changes the activation policy. Grepped; the only
  `set_activation_policy` calls in the crate are `setup`'s `Accessory` and the settings
  window's `Regular`/`Accessory` pair, none of which is on this path.
- `ActivationPolicy::Accessory` does not prevent this. Apple: accessory "doesn't appear
  in the Dock and doesn't have a menu bar, but it **may be activated programmatically**"
  — the policy removes the Dock icon, not activation.
- The activation is a **single event**, not a sticky state: re-activating Finder 2 s
  after the service call from the probe harness stuck, and Aloud never took it back.

Because Aloud is an accessory app with no ordinary windows, "active" means the user's
key window has resigned key and their typing goes nowhere — which is precisely the
reported symptom.

**The consequence for a fix:** activation cannot be prevented from inside Aloud, only
relinquished. That is a real limit of where the app sits in this flow, not a shortcut.

---

## 3. What did not work: `NSApp.deactivate()`

Tried first, because it is the obvious call. It does nothing here.

- Called inline at the top of the handler: `isActive` stayed `true` at +30/100/300/800/2000 ms.
- Re-scheduled via `performSelector:withObject:afterDelay:` at 50/200/500/1000 ms, in
  case the in-flight activation was overriding a too-early call: `isActive` still `true`
  throughout, frontmost still Aloud.

Consistent with Apple's own description of it — abstract "Deactivates the receiver",
discussion "Normally, you shouldn't invoke this method—AppKit is responsible for proper
deactivation." It drops the app's own active state and nothing else, so under
macOS 14+'s cooperative activation there is no one to hand activation to.

## 4. The fix: `NSApp.hide(nil)`, first thing in the Service handler

`src/selection/macos.rs`, at the top of `read_selection`, before the pasteboard is even
read and before every early return — because the focus is already gone by then whether
or not the text turns out to be worth speaking.

Apple's abstract for `hide(_:)` is the whole reason it works where `deactivate()` did
not: *"Hides all the receiver's windows, and the next app in line is activated."* It
does not merely drop our active state, it hands activation on.

Hiding is **unconditional**, including when the settings window happens to be open. The
rule is "on a selection read, Aloud always gets out of the way." Skipping the hide while
a window is visible would leave the *worse* variant of this bug in place — the settings
window jumping in front of the user's work on every ⌘⇧A. The accepted cost is that
reading a selection made inside the settings window hides that window; it is reopened
from the tray.

### Verified after the fix

| Check | Result |
|---|---|
| App already running, service invoked | frontmost stayed Finder, **0 changes observed** over 15 s |
| App **not** running, launched by the service (the tao `activateIgnoringOtherApps` path) | frontmost stayed Finder, **0 changes observed**; `app start` in the log confirms the cold launch |
| Text still spoken | `speak_selection: completed, spoke` on both runs |
| Tray status item survives the hide | present in the menu bar (the "A" speech-bubble template icon) after a hide |
| Settings window still opens afterwards | opens, key and frontmost — Tauri's `show()` + `set_focus()` unhides the app; verified by screenshot on the instrumented build with the identical `hide()` call |
| `cargo test --release` | all green, 0 failed |

**No Accessibility grant is involved.** The fix is one AppKit call inside our own
process; the hard constraint that Aloud never requires Accessibility is untouched, as is
the `NSServices` declaration and every seam around it.

---

## 5. Reproduction harness

Kept out of the repo deliberately (it drives the real installed bundle, it is not a
unit test), but it is four lines of AppKit and worth having written down. Compile with
`swiftc -O probe.swift -o probe`, run with the app installed:

```swift
import AppKit

let finder = NSRunningApplication
    .runningApplications(withBundleIdentifier: "com.apple.finder").first!
finder.activate()
Thread.sleep(forTimeInterval: 1.5)

var last = NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"
print("BEFORE \(last)")

let pb = NSPasteboard(name: NSPasteboard.Name("AloudFocusProbe"))
pb.clearContents()
pb.declareTypes([.string], owner: nil)
pb.setString("Focus probe.", forType: .string)
print("NSPerformService -> \(NSPerformService("Read Aloud", pb))")

let deadline = Date().addingTimeInterval(15)
while Date() < deadline {
    RunLoop.current.run(until: Date().addingTimeInterval(0.1))
    let now = NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"
    if now != last { print("CHANGE \(now)"); last = now }
}
```

`NSPerformService` is the sanctioned stand-in for the keystroke: it is the same AppKit
Services dispatch the ⌘⇧A key equivalent runs, and it needs no Accessibility grant and
no synthetic keystrokes. The one thing it does not prove on its own is that the *sender*
side behaves identically for a real menu-driven invocation — but that does not matter
for the mechanism, because the activation is already committed inside Aloud's process
before its handler runs, whoever sent the request.

## 6. What is not covered by a regression test

Nothing here is unit-testable: it needs an installed, signed `.app`, a live Services
registration, and a second application to hold focus. The check to re-run by hand after
touching `src/selection/macos.rs` is the harness above — 0 frontmost changes is the pass
condition.
