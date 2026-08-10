# Media-key pause/resume for Aloud — platform research

Researched 2026-08-10 by PAX, on this M1 Air, **macOS 26.6 (25G72)**.

**The question:** can Aloud be paused and resumed with the play/pause media key on the MacBook
keyboard, without ever asking for Accessibility?

**Short answer: yes — but not by the route the current code is defending against.** The
`CGEventTap` path is now settled and it is settled *against* us. The Apple-sanctioned route
(`MPRemoteCommandCenter`) needs no TCC grant at all. And the audio layer cannot pause at all today,
which is the real gating work regardless of which key triggers it.

> **OUTCOME, 2026-08-10 — the recommendation was followed, and the media key was then DROPPED.
> Read this before acting on anything below.** Both halves of the research were carried out.
> The audio-layer work (§5) shipped and stands. The `MPRemoteCommandCenter` route (§1-§3) was
> implemented as Phase 2 — F8 routed to the existing `toggle_pause`, `objc2-media-player` 0.3.2,
> next/previous/seek explicitly disabled, no user text in `nowPlayingInfo` — and it cleared every
> programmatic check, including `mediaremoted` logging a handover in both directions.
>
> **Then the owner pressed the key, and the one test §2 called decisive failed: when a read
> ended, control did not return to Music.app.** That is precisely the "worse than not having the
> feature" outcome §2 names — the owner's music controls stay broken after every read — so the
> feature was dropped rather than shipped. Note the shape of this: the programmatic evidence said
> the hand-back worked, and pressing the key by hand said otherwise. §2 was right to insist on
> the physical test and right that fifteen minutes of it settles the question.
>
> The work is preserved, unmerged, at tag `experiment/media-key-mpremote` (parent `00d044f`);
> its report `docs/2026-08-10-media-key-phase2.md` exists **only on that tag**. On `main` Aloud
> binds no media keys by any mechanism. Pause is ⌘⇧A and the tray item.
>
> What survives as durable: §4's `CGEventTap` measurement (the denylist's justification),
> §3's crate reachability facts, and §6's "simpler alternatives", both of which shipped.

Tags: **VERIFIED** = primary source cited, or executed here · **LIKELY** = named evidence, not
conclusive · **UNVERIFIED** = says what would settle it.

---

## 1. `MPRemoteCommandCenter` / `MPNowPlayingInfoCenter` — the sanctioned route

- **VERIFIED — `MPRemoteCommandCenter` is a real macOS API since 10.12.2.** Apple's documentation
  platform metadata lists `macOS 10.12.2` alongside iOS 7.1. Corroborated by the macOS Sierra
  release notes: *"The MediaPlayer API is now available on macOS."*
  https://developer.apple.com/documentation/mediaplayer/mpremotecommandcenter ·
  https://developer.apple.com/library/archive/releasenotes/MacOSX/WhatsNewInOSX/Articles/macOSSierra10.12.html
- **LIKELY — no TCC grant, no entitlement, no `Info.plist` key.** Apple's docs for
  `MPRemoteCommandCenter`, `MPNowPlayingInfoCenter`, and `playbackState` contain no mention of
  entitlement, privacy, authorization, or usage description. This is an argument from documented
  absence, but a strong one: the API is a registration/callback surface, not an event tap, so it
  never touches the gates that `CGEventTap` and `NSEvent.addGlobalMonitorForEvents` do.
- **VERIFIED — the one entitlement in this area is Apple-private and iOS-only.**
  `com.apple.mediaremote.set-playback-state` gates `playbackState` on **iOS**, not macOS. Apple DTS
  on that thread confirms `com.apple.` entitlements are unavailable to third parties, and that
  `playbackState` is a macOS API. https://developer.apple.com/forums/thread/678108
- **VERIFIED — `playbackState` is the macOS-specific requirement, and it is mandatory.** The
  property's Discussion reads *"This property only applies to macOS"* and *"You must set this
  property every time the app begins or halts playback."* Note the trap: the *availability*
  annotation is cross-platform (iOS 13, tvOS 13, macOS 10.12.2) — it is only the *behaviour* that
  is macOS-only. https://developer.apple.com/documentation/mediaplayer/mpnowplayinginfocenter/playbackstate
- **VERIFIED — `AVAudioSession` does not exist on macOS.** Its platform list has no macOS entry
  (iOS, iPadOS, Mac Catalyst, tvOS, visionOS, watchOS only).
  https://developer.apple.com/documentation/avfaudio/avaudiosession
  **The macOS equivalent of `setActive(true)` is exactly `MPNowPlayingInfoCenter.playbackState`.**
  The Sierra release notes give the reason: macOS has no central media server to infer state from,
  so the app reports it. WWDC19 session 501 says the same — on macOS you set the playback state
  instead of activating an audio session.
- **LIKELY — no AVFoundation requirement; raw CoreAudio/`rodio` output is fine.** No Apple document
  conditions Now Playing status on AVFoundation; the macOS mechanism is registration plus
  `playbackState`, and the system takes your word for the state. Strongest corroboration:
  [`russellhancox/btrcd`](https://github.com/russellhancox/btrcd) is a macOS app that plays **no
  audio at all** and exists solely to receive `MPRemoteCommandCenter` events.
- **LIKELY — the app must actually be playing, not merely registered.** WWDC19 501: you become the
  Now Playing app by supporting at least one remote command *and* initiating playback. WWDC22
  110338 describes system *"heuristics to qualify apps as Now Playing eligible"*, the first being
  registering a handler for at least one command.
- **LIKELY — `LSUIElement` / accessory apps work.** `btrcd` ships `LSUIElement = true`, has no UI,
  and works. No report anywhere of `MPRemoteCommandCenter` failing *because* an app is accessory.
  ⚠ One caveat with teeth: btrcd's README notes it must be a real `.app` bundle — as a bare binary
  it did not receive next/previous events. **Test `target/Aloud.app`, never `cargo run`**, or you
  get a false negative. (btrcd's own testing was 10.12.3, circa 2017.)
- **UNVERIFIED — whether ad-hoc / self-signed ("Aloud Dev") signing changes anything.** No evidence
  either way; no report ties `MPRemoteCommandCenter` failure to signing identity, and the only
  entitlement in play is Apple-private and iOS-scoped. **What settles it:** run the bundled app,
  set `playbackState = .playing`, and watch Console for
  `[NowPlaying] [MRNowPlaying] Ignoring setPlaybackState because application does not contain
  entitlement…`. Absence of that line = settled. Ten minutes.

**What this means for Aloud.** The sanctioned route exists, is available on every macOS Aloud
targets, and asks for nothing — no prompt, no entitlement, no plist key, no AVFoundation. The
required shape is: register at least one command on `MPRemoteCommandCenter.shared()`, populate
`MPNowPlayingInfoCenter.nowPlayingInfo`, and set `playbackState` on **every** start and stop. The
"no Accessibility, ever" constraint is not violated. Two small risks remain (ad-hoc signing, bundle
vs bare binary) and one short empirical test clears both.

---

## 2. What it costs the user — the media-key takeover

- **VERIFIED — relinquishing is `playbackState = .stopped`, and it is not what you would guess.**
  From Apple's own *Becoming a Now Playable App* sample (downloaded from
  `docs-assets.developer.apple.com/published/6eacaf8a4885/BecomingANowPlayableApp.zip`), the
  `NowPlayable` protocol comment on `handleNowPlayableSessionEnd()` says it exists *"to allow other
  apps to become the current NowPlayable app"*, and the macOS implementation is one line:
  `MPNowPlayingInfoCenter.default().playbackState = .stopped`.
  Neither alternative is the documented route: `nowPlayingInfo = nil` is documented only as
  clearing the metadata dictionary, and `command.isEnabled = false` is documented only as
  *"events for this command are not sent to your app"* — per-command muting, not session release.
- **LIKELY — registering + playing does take the keys.** Apple forum thread 685333's premise is
  that before `playbackState` was set, the media keys launched Music.app instead; after setting it,
  the app appeared in Now Playing and the controls worked.
  https://developer.apple.com/forums/thread/685333
- **LIKELY — a single system-wide owner, tied to active playback rather than to the frontmost app.**
  A 2025 forum report describes a macOS app that lost "audio focus" being unable to update its own
  Now Playing state until the focus-holder changed state. One participant, zero replies — treat as a
  single observation. https://developer.apple.com/forums/thread/799691
- **UNVERIFIED — the arbitration algorithm itself.** Apple documents it nowhere. The widely-repeated
  "most recent source wins" claim appears only in SEO-grade blog content and is deliberately not
  cited here. The only Apple-published timeout is UI-scoped, not key-routing-scoped: the Touch Bar
  Now Playing button disappears after eight minutes without media.
- **UNVERIFIED, and this is the decisive one — does releasing hand the keys *back* to Spotify, or
  leave them dead?** The 685333 premise hints that "dead" may actually mean "Music.app launches",
  which would be a bad outcome. **What settles it:** start Spotify playing, trigger an Aloud read,
  press play/pause (expect: Aloud responds, Spotify does not), let Aloud finish so it sets
  `.stopped`, press play/pause again — and observe whether Spotify resumes, nothing happens, or
  Music.app launches. Fifteen minutes with the built app.

**What this means for Aloud.** The takeover is real but **scoped and self-releasing**: Aloud would
hold the play/pause key only while it is actually speaking, and hand it back the moment a read ends.
That is defensible — the key controls whatever is playing *now*. The unacceptable failure mode is
not the takeover, it is the hand-*back*: if releasing leaves the key dead or launches Music.app, the
owner's music is worse off after every read. **Do not ship this feature until that one test has been
run.** It is cheap, and it is the whole UX risk.

> **RESOLVED 2026-08-10 — the test was run by hand, and it came back bad.** Phase 2 was built and
> the owner pressed the key. After a read ended, **control never returned to Music.app** — the
> exact failure this paragraph gates on. The feature was dropped, not shipped. This section's
> judgement was correct in every part: the risk was the hand-back and nothing else, and the
> decisive evidence was a human pressing a key, not a log line. `mediaremoted` had recorded a
> handover in both directions during the programmatic pass, which is why the physical test was
> not redundant. Preserved unmerged at tag `experiment/media-key-mpremote`.

---

## 3. Rust reachability — `objc2-media-player`

- **VERIFIED — `objc2-media-player` 0.3.2 exists and is version-compatible with Aloud today.**
  Published 2025-10-04, 28 KB, `Zlib OR Apache-2.0 OR MIT`, MSRV 1.71.
  Its dependency requirements vs. what Aloud's `Cargo.lock` already resolves:

  | Requirement | Aloud has | OK |
  |---|---|---|
  | `objc2 >=0.6.2, <0.8.0` | 0.6.4 | ✅ |
  | `objc2-foundation ^0.3.2` | 0.3.2 | ✅ |
  | `objc2-app-kit ^0.3.2` (optional) | 0.3.2 | ✅ |
  | `block2 >=0.6.1, <0.8.0` (optional) | 0.6.2 | ✅ |

  **Zero new transitive ecosystem.** Every dependency it needs is already in the lockfile.
- **VERIFIED — it exposes exactly the API surface required**, each behind its own cargo feature
  (43 features total, so the existing `default-features = false` discipline carries over):
  - `MPRemoteCommandCenter` → `sharedCommandCenter`, `playCommand`, `pauseCommand`,
    `togglePlayPauseCommand`, `stopCommand`, `nextTrackCommand`, …
  - `MPRemoteCommand` → `addTargetWithHandler`, `setEnabled`, `removeTarget`
  - `MPNowPlayingInfoCenter` → `defaultCenter`, `nowPlayingInfo`, `setNowPlayingInfo`,
    `playbackState`, `setPlaybackState`
  - plus `MPRemoteCommandEvent`, `MPRemoteControlTypes` (handler-status and playback-state enums)
  Verified from docs.rs for 0.3.2, not assumed.
- **VERIFIED — `souvlaki` 0.8.3 is the wrong choice here, despite being the obvious one.** It is the
  established cross-platform media-key crate (204k downloads), but on macOS it depends on the
  *previous-generation* `objc 0.2.7` + `cocoa 0.24` + `core-graphics 0.22` stack. Adopting it means
  carrying two independent Objective-C binding ecosystems in one binary alongside Aloud's
  `objc2 0.6.4`. Rejected on dependency hygiene, not quality.
- **Hand-rolling is not needed, but the fallback is cheap anyway**: `extern_class!` + `msg_send!`
  for two classes and three or four selectors is well under 100 lines, and Aloud already does this
  shape of work for the NSServices provider.
- ⚠ **VERIFIED forward risk, not an immediate one.** A new **NowPlaying framework** landed
  (WWDC26 session 312) but is **macOS 27.0+ only**, and its docs explicitly warn that mixing it
  with the MediaPlayer APIs for local playback *"results in undefined behavior."* If Aloud ever
  adopts it, it is a migration, not an addition. https://developer.apple.com/documentation/nowplaying

**What this means for Aloud.** The binding cost is essentially nil: one 28 KB crate, four features,
no new transitive deps, same `objc2` generation and same `default-features = false` style already in
`Cargo.toml`. This is not the expensive part of the feature.

---

## 4. The `CGEventTap` route — the open question, settled, in the negative

The M4 research left this **UNVERIFIED**: the tap masks `SystemDefined` rather than key events, so
the *documented* gate arguably does not apply. **It is now settled, and the answer is worse than
"gated" — the discriminator is not the mask at all, it is the tap *option*.**

- **VERIFIED BY DIRECT TEST on this machine, macOS 26.6 (25G72), 2026-08-10**, from a process with
  `AXIsProcessTrusted=false`, `CGPreflightListenEventAccess=false`, `CGPreflightPostEventAccess=false`,
  `IOHIDCheckAccess(ListenEvent)=DENIED`. All taps at `kCGSessionEventTap` / `kCGHeadInsertEventTap`:

  | Mask | Option | Result |
  |---|---|---|
  | `keyDown\|keyUp` | listen-only | `NULL` |
  | `keyDown\|SYSDEFINED` | listen-only | granted mask `0x4000` (keyDown bit cleared), **`enabled=false`** |
  | `SYSDEFINED` only | listen-only | granted `0x4000`, **`enabled=false`** |
  | **`SYSDEFINED` only** | **active (`kCGEventTapOptionDefault`)** | **`NULL`** |
  | `mouseMoved` only | active | `NULL` |

  Three things this settles. (a) The M4 doc's reading was **right**: `NX_SYSDEFINED` genuinely is
  exempt from the key-event gate — the `keyDown` bit was cleared while bit 14 survived. (b) But the
  listen-only tap is created **disabled**, so it delivers nothing without **Input Monitoring**
  (`kTCCServiceListenEvent`). Control: `CGGetEventTapList` on this machine shows *other* processes'
  listen-only taps `enabled=TRUE`, so `enabled=false` is a real refusal, not an artifact. (c) An
  **active** tap returns `NULL` regardless of mask — `mouseMoved` fails identically. It is an
  option-level gate, not a keyboard-specific one, and it maps to the **Accessibility** pane.
- **VERIFIED — Apple's header still documents the bit-clearing, and it is live.** `CGEvent.h:272-279`
  (SDK 26.5): *"the appropriate bits in the mask are cleared. If that results in an empty mask, then
  NULL is returned."* `CGEventTapCreate` carries no deprecation attribute.
- **VERIFIED — Apple DTS names the split explicitly.** Quinn "The Eskimo!",
  https://developer.apple.com/forums/thread/707680 : *"the former requires the Accessibility
  privilege whereas the latter requires the Input Monitoring privilege"* (his sample uses
  `.listenOnly`). And https://developer.apple.com/forums/thread/735204 (Aug 2023): listen access
  surfaces as Input Monitoring, PostEvent access as Accessibility — DTS's own reply: *"Yep. That is
  super confusing."*
- 🚩 **VERIFIED — `global-hotkey` 0.8.0 uses the ACTIVE option, and upstream says it prompts.**
  `src/platform_impl/macos/mod.rs:205-210` passes `CGEventTapOptions::Default`. In
  [tauri-apps/global-hotkey PR #71](https://github.com/tauri-apps/global-hotkey/pull/71)
  (2024-04-17) the author states: *"this will trigger OS to request `Accessibility` permission"*,
  and next day: *"It'll pop up automatically. Currently, it will only ask when registering media
  keys."* PR #72's body repeats it.
- **VERIFIED — Chromium hit a genuinely dangerous edge with the active tap.**
  `ui/base/accelerators/media_keys_listener_mac.mm`, commit 2026-05-25 (`7474294`): *"When a
  CGEventTap is created with kCGEventTapOptionDefault, revoking Accessibility permission at runtime
  can cause the system to hang and become unresponsive to all input."* Chromium moved
  `MPRemoteCommandCenter` to primary back in 2019 and keeps the tap only as a secondary listener.
- **LIKELY — self-signing makes an event tap's grant unstable anyway.** TCC keys grants to code
  identity, so `codesign --sign -` mints a new cdhash on every build and resets the grant. A
  documented failure mode (2026-02-18) is that after re-signing the tap installs, reports enabled,
  and never fires — *"a non-nil tap is not a healthy tap"* — with no `kCGEventTapDisabledBy*`
  callback. Aloud is self-signed with "Aloud Dev", so it sits squarely in this.

**What this means for Aloud.** `src/shortcut.rs`'s `MEDIA_KEYS` denylist is **correct and now
empirically justified** — upgrade its doc comment from "could produce a prompt" to "does produce an
Accessibility prompt (upstream PR #71), and the tap returns NULL without the grant (measured here,
macOS 26.6)". The same applies to `docs/M4-platform-research-macos.md:522-526`, which currently says
the gate is *"arguably not triggered (LIKELY, not verified)"* — that line is now superseded and
should be corrected in the same change set as any work here. The honest verdict on the tap route:
even the listen-only variant, which needs "only" Input Monitoring, is unacceptable — it is still a
TCC prompt, it still cannot suppress the key from also reaching Spotify, and Aloud's self-signing
would make the grant evaporate on every rebuild. **The tap is not a route to this feature. Do not
reconsider it.**

---

## 5. Does pause/resume even exist in the audio layer? — no, and this is the real work

> **SUPERSEDED 2026-08-10 — this section's premise no longer holds. Phase 1 built it.**
> Pause/resume now exists behind the `AudioSink` seam, the stall-guard trap named below is
> fixed, and both are shipped. Phase 1 also gave pause its own ordinary `Cmd+Shift+P` chord;
> that chord was **removed** the same day as redundant surface, since `⌘⇧A` had by then become a
> selection-aware play/pause toggle. Pause is reached by `⌘⇧A` and the tray item. Every
> prediction in this section held up under implementation, including the 30s stall trap, which
> was reproduced as a failing test before being fixed. The section is kept as written — it was
> the research that motivated the work — but read
> [`2026-08-10-pause-resume-phase1.md`](2026-08-10-pause-resume-phase1.md) for what was actually
> built and what it measured. §1-§4 and §6 are unaffected; the media-key half of §1/§2 remains
> unbuilt and still gated on the untested Spotify hand-back behaviour.

- **VERIFIED — `rodio` 0.22.2 supports it.** `src/player.rs` on the exact vendored version:
  `pause()` (*"Pauses playback of this player. No effect if already paused. A paused sink can be
  resumed with `play()`"*), `play()` (*"Resumes playback of a paused player"*), and `is_paused()`.
  Pause acts on the output side, so it is sample-accurate and works **mid-sentence**.
- **VERIFIED — Aloud cannot reach any of it today.** `src/play/sink.rs`'s `AudioSink` trait is
  exactly `append` / `queued` / `stop`. `src/play/player.rs` has `stop()` and nothing else. Pause
  and resume do not exist anywhere in the crate.
- **VERIFIED — a naive pause would break after 30 seconds.** `Player::wait_for_drain`
  (`src/play/player.rs:112-139`) bails with *"audio device appears stalled"* if `sink.queued()` has
  not changed for `STALL_TIMEOUT` (30s). A paused sink has a frozen queue depth **by definition**,
  so any pause longer than 30 seconds trips the stall guard and aborts the read. This is a genuine
  trap: the guard cannot distinguish "device died" from "user paused". It must become pause-aware
  (freeze the clock while paused), not have its timeout raised.
- **VERIFIED — pause is cheap and does not burn CPU.** `run()` keeps at most one sentence buffered
  ahead (`wait_for_drain(2)`), so while paused the synthesis thread simply sits in the drain loop
  rather than synthesising. And because `rodio`'s `pause()` acts on the sink directly, pause takes
  effect immediately even while the synthesis thread is blocked inside `engine.synthesize()` —
  the same property that makes the existing `stop()` responsive.

**What this means for Aloud.** *"A media key that stops but cannot resume is not the feature the
owner asked for"* — and today Aloud can only stop. The good news is that real, mid-sentence,
sample-accurate pause/resume **is** achievable, and the work is small and self-contained: two new
`AudioSink` trait methods (plus the test double), a `Player::pause`/`resume` pair guarding a
paused-state flag, and a pause-aware fix to the stall guard. **This work is required no matter which
control surface triggers it** — media key, ordinary hotkey, or tray item. It is the prerequisite,
and it carries none of the platform risk.

---

## 6. Simpler alternatives worth naming

- **An ordinary (non-media) global hotkey — e.g. `Cmd+Alt+P`.** Goes through the existing Carbon
  `RegisterEventHotKey` path, which M4 already **VERIFIED** needs no Accessibility grant
  (`global-hotkey-0.8.0/src/platform_impl/macos/mod.rs:116-123`, `inOptions = 0`). No new crate, no
  new framework, no tap, no TCC, no Now Playing arbitration. It also *keeps working while Spotify is
  playing*, because it never contends for the media key at all. The infrastructure exists: Aloud
  already records chords in the settings webview, validates them in `src/shortcut.rs`, and applies
  them via `apply_shortcut_with`. `Settings` currently holds a single `region_shortcut`; this adds a
  second field and a second registration. Cost is roughly a day, most of it the audio-layer work
  from §5 that the media-key route needs too.
- **A tray "Pause / Resume" item.** `src/bin/aloud.rs:826` already builds a `Stop` item and handles
  it at :866. A pause item is a handful of lines plus the label toggling that `refresh_tray_labels`
  already does for other items. Effectively free once §5 exists. Not a good *primary* control — it
  costs a trip to the menubar — but it is the right discoverable fallback and a fine first
  increment.
- **Control Centre integration is not a separate option.** It is the *same* `MPNowPlayingInfoCenter`
  work as §1: populate `nowPlayingInfo` and Aloud appears in Control Centre's Now Playing module for
  free. Worth knowing it comes as a package deal — you cannot take the media key without also
  appearing there, and appearing there is arguably a feature.

**What this means for Aloud.** There is a real cheap option, and it is not a consolation prize: an
ordinary hotkey gives the owner pause/resume with zero platform risk and zero contention with his
music. The media key is strictly an ergonomics upgrade layered on top — a nicer key, at the price of
a framework, a Now Playing session, and one unverified hand-back behaviour.

---

## Recommendation

*A proposal. The owner decides.*

> **He decided, 2026-08-10: point 4's cheap alternative shipped; points 1-3's media key was built
> and then dropped.** The §2 hand-back test failed under the owner's hand — control never went
> back to Music.app when a read ended — so point 3's UNVERIFIED is now a VERIFIED *against*.
> The recommendation's own logic delivers the outcome: "if that test is bad, the owner has lost
> nothing and still has working pause/resume." That is exactly where the project stands. The
> paragraphs below are the proposal as written; do not read them as an open plan.

**1. Yes — the media key can pause and resume Aloud without an Accessibility prompt, but only via
`MPRemoteCommandCenter`, never via an event tap.** The tap question that M4 left open is now
settled against it: the gate is the tap *option*, not the mask, and `global-hotkey` 0.8.0 uses the
active option that upstream confirms prompts for Accessibility. Even the listen-only variant needs
Input Monitoring, still cannot suppress the key from Spotify, and would lose its grant on every
rebuild under Aloud's self-signed identity. The `MEDIA_KEYS` denylist stays; its justification gets
stronger, and `docs/M4-platform-research-macos.md` §3's media-key bullet needs correcting —
**done 2026-08-10**, it now carries a CORRECTED block pointing back here (cited by section rather
than by line number, since line numbers move).

**2. What it costs.** Complexity is modest and lands in three places: one 28 KB crate
(`objc2-media-player` 0.3.2, already version-compatible, zero new transitive deps); roughly 100 lines
wiring `togglePlayPauseCommand` and `playbackState`; and the §5 audio-layer work. Two risks are
unresolved and one cheap test clears both — build `target/Aloud.app` (not `cargo run`), set
`playbackState = .playing`, watch Console for an entitlement complaint, then run the Spotify
hand-back experiment from §2. Call it 30 minutes before committing to any of it.

**3. The media-key takeover, in one line.** Aloud would hold the play/pause key only while it is
actually speaking and release it via `playbackState = .stopped` when a read ends — so it never
permanently hijacks the key, but **whether releasing hands control back to Spotify or leaves the key
dead (possibly launching Music.app) is UNVERIFIED**, and that is the one thing that would make this
worse than not having the feature.

**4. The cheap alternative, and the suggested order.** ✅ **Done 2026-08-10 (Phase 1)** — the
audio-layer work, the stall-guard fix, the ordinary hotkey and the tray item all shipped; see
[`2026-08-10-pause-resume-phase1.md`](2026-08-10-pause-resume-phase1.md).

> **CORRECTED 2026-08-10 — the sentence that stood here is no longer true.** It read: *"The
> `MPRemoteCommandCenter` step below remains open and still gated on the §2 hand-back test."*
> It was written in the hours between Phase 1 shipping and Phase 2 being built, and it was
> overtaken the same day. **The step was taken and the answer came back bad:** Phase 2 was built,
> the owner ran the §2 test by hand, control never returned to Music.app when a read ended, and
> the media key was **dropped**. Nothing below this line is an open plan — read the top banner,
> §2's RESOLVED note and `CLAUDE.md` constraint 15(a) before acting on any of it. One further
> dating, for a reader who lands here first: the ordinary chord in the ✅ line above was `⌘⇧P`,
> and it too was removed the same day (see §5's note). Pause is `⌘⇧A` and the tray item.

Original recommendation follows. Do the §5 audio-layer pause/resume first —
it is required by every option, carries no platform risk, and includes a real latent bug (the 30s
stall guard aborting any paused read). Ship it behind an ordinary global hotkey plus a tray
Pause/Resume item: no new framework, no TCC, no Now Playing arbitration, and it keeps working while
the owner's music plays. Then treat `MPRemoteCommandCenter` as a separate, additive step, gated on
the Spotify hand-back test coming back clean. If that test is bad, the owner has lost nothing and
still has working pause/resume.
