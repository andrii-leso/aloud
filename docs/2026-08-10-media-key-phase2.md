# Media key (F8), Phase 2 — what was built, what was measured, and the gate

FORGE, 2026-08-10. Branch `feat/pause-resume`, in the isolated worktree, on top of Phase 1
(`00d044f`). macOS 26.6 (25G72), M1 Air.

**Verdict so far: every kill criterion is clear, and the one the research called decisive came
back clean.** No Accessibility prompt is possible — the route is a registration surface, not an
event tap, and the built bundle carries no entitlements and no new TCC symbol. Self-signing
turned out not to matter. And control **does** return to Music.app when a read ends: `mediaremoted`
logged the handover in both directions.

**What is still open, and it is deliberately open:** nobody has physically pressed F8. That is
the owner's five-minute script in §5. I am not recommending a merge before he runs it.

Scope: this adds a **second trigger** for the `toggle_pause` Phase 1 already built. No new
playback machinery, no change to the pause implementation, no change to `MEDIA_KEYS`.

---

## 1. What was implemented

| File | What |
|---|---|
| `src/now_playing/mod.rs` | The seam: `PlaybackState` (Playing/Paused/Stopped), the `NowPlaying` trait, a `Silent` no-op default, `RemoteCommand`, and `should_toggle` — the one routing decision, pure and testable |
| `src/now_playing/macos.rs` | `MediaRemote`: `MPNowPlayingInfoCenter.playbackState` + `nowPlayingInfo`; `install_remote_commands` registering play / pause / togglePlayPause and **disabling** next / previous / seek |
| `src/play/player.rs` | Publishes state from its own ground truth at the real session boundary; `SpeakSession` makes the release unconditional |
| `src/bin/aloud.rs` | Builds `MediaRemote`, hands it to the `Player` (and to the voice-swap replacement), installs the commands in `setup()`, routes them through `handle_remote_command` |
| `Cargo.toml` | `objc2-media-player` 0.3.2 + `block2` 0.6.2, both macOS-gated |
| `tests/now_playing.rs` | 9 new tests |

**Where the session boundary is, and why it matters.** `Playing` is published at the top of
`Player::speak` and `Stopped` on every exit from it. Not at the top of `read_region`: that would
claim the media key during the region drag and the OCR pass, seconds before any audio, and hand
it back again on an `Escape` cancel. `Player::speak` is the only place that is true.

**`Paused`, not `Stopped`, on pause.** Pausing keeps the Now Playing session so the *next* press
resumes Aloud. Publishing `Stopped` there would release the key mid-read and make resume
impossible from the key that paused it.

**The release is a `Drop`, not a trailing line.** `run()` calls `TtsEngine::synthesize` over
whatever text happened to be on screen. On a panic unwind, a trailing `speaking.store(false)`
is simply skipped — and *that* is what "holds the key forever with nothing to play" looks like
in practice. `SpeakSession`'s `Drop` fires on ordinary return, on `?`, and on an unwind alike.

> **A pre-existing behaviour change, named rather than folded in.** Before this, a panic inside
> `run()` left `speaking == true` for the life of the process, so every later `Player::pause()`
> would accept a pause with nothing playing. `SpeakSession` clears it. This is a fix, it was not
> asked for, and it is here because the media-key release depends on the same store —
> `a_panicking_read_still_gives_the_session_back` pins both halves.

**Directional commands are idempotent.** `Play` resumes only if paused; `Pause` pauses only if
playing; only `TogglePlayPause` (what F8 sends) flips unconditionally. If macOS ever delivered
both a directional command *and* the toggle for one key press, a blind toggle would flip twice
and the key would look dead. `Play` while idle does nothing at all — the media key must never
*start* a read.

**Next / previous / seek are explicitly disabled.** A Now Playing app receives the whole
transport, not only what it registered. Left at their defaults, F7 and F9 would route to Aloud
and silently do nothing while it speaks. That is a regression in the owner's music controls, and
it is exactly the "worse than not having it" outcome this phase is gated against.

**No user text is published.** `nowPlayingInfo` carries the fixed string `"Reading aloud"`.
`nowPlayingInfo` renders in Control Centre and can be relayed to other devices; putting the
captured screen text there would leak the content of a deliberately-local app into a system-wide
surface. (§3 also shows the metadata is not load-bearing for the media key at all.)

### The dependency claim, checked rather than trusted

`objc2-media-player` 0.3.2 resolved to **exactly one new package and zero new transitive
dependencies** — the whole `Cargo.lock` delta is the one `[[package]]` block, whose `dependencies`
are `bitflags`, `block2`, `objc2`, `objc2-foundation`, all already present. Features: 8 of the
crate's 43, the minimum `macos.rs` actually reaches. `souvlaki` was not used, per the brief and
for the stated reason (it would drag the legacy `objc`/`cocoa` stack in alongside `objc2`).

---

## 2. What was verified programmatically, on the built bundle

`target/Aloud.app`, built by `packaging/make-app.sh`, signed "Aloud Dev".

```
=== ENTITLEMENTS ===            (empty — the bundle has none)
=== MediaPlayer linked? ===
	/System/Library/Frameworks/MediaPlayer.framework/Versions/A/MediaPlayer
=== event-tap / TCC symbols in the binary ===
CGEventTapCreate                 PRESENT
CGEventTapEnable                 PRESENT
AXIsProcessTrusted               absent
AXIsProcessTrustedWithOptions    absent
CGRequestListenEventAccess       absent
IOHIDRequestAccess               absent
=== privacy / usage keys in bundle plist ===
(none)
```

**On `CGEventTapCreate` being PRESENT — read this before drawing a conclusion.** It is an
undefined symbol imported by `global-hotkey`, and it is **pre-existing**: the same two symbols
are in the owner's currently-installed `/Applications/Aloud.app`, built from `main` before any
of this work. The only call site is `start_watching_media_keys`, reachable only by *binding* a
media key as a hotkey — which `MEDIA_KEYS` rejects at both entry points. Phase 2 adds no tap, no
new TCC symbol, and no entitlement. A symbol table cannot prove a call site is unreachable; the
`MEDIA_KEYS` tests do that, and they still pass.

### Test suite

`cargo test --release`, full run: **179 passed, 0 failed, 3 ignored** across 18 binaries
(Phase 1 was 170/0/3; the 9 new ones are `tests/now_playing.rs`).

```
     Running unittests src/lib.rs
running 77 tests
test result: ok. 77 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.59s
     Running unittests src/bin/aloud.rs
running 29 tests
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.02s
     Running unittests src/bin/aloud_say.rs
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/actions.rs
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
     Running tests/engine_smoke.rs
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.66s
     Running tests/engine_speed_pin.rs
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.29s
     Running tests/engine_thread.rs
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.21s
     Running tests/latency_budget.rs
running 2 tests
test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 14.60s
     Running tests/now_playing.rs
running 9 tests
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
     Running tests/ocr_macos.rs
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.93s
     Running tests/player_pause.rs
running 7 tests
test result: ok. 6 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.79s
     Running tests/player_stop.rs
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
     Running tests/selection.rs
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/settings.rs
running 14 tests
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/shortcut.rs
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/shortcut_apply.rs
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/speed_preserves_words.rs
running 1 test
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/voice_swap.rs
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.45s
   Doc-tests aloud
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`src/play/` changed, so the `#[ignore]`d real-device test was run explicitly, per hard
constraint 13:

```
running 1 test
test rodio_really_pauses_without_discarding_buffers ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 7.30s
```

New coverage:

| Test | Pins |
|---|---|
| `a_read_takes_the_session_and_gives_it_back` | exactly `[Playing, Stopped]` for a whole read |
| `an_empty_text_never_takes_the_session` | nothing spoken ⇒ the key is never taken at all |
| `a_failed_read_still_gives_the_session_back` | an `Err` unwind releases |
| `a_panicking_read_still_gives_the_session_back` | a panic releases, and clears `speaking` |
| `pausing_and_resuming_a_live_read_keeps_the_session` | pause ⇒ `Paused` (not `Stopped`), resume ⇒ `Playing`, end ⇒ `Stopped` |
| `pausing_with_nothing_speaking_publishes_nothing` | a refused pause publishes nothing |
| `toggle_always_flips` / `directional_commands_are_idempotent` / `play_while_idle_does_not_start_a_read` | the routing decision |

`cargo clippy --release --all-targets`: 6 warnings, all pre-existing, all in `src/text/chunk.rs`
and `src/vendor/` — files this change does not touch. Unchanged from Phase 1. `cargo fmt` applied
to the new code only (it wanted to reformat several untouched pre-existing blocks in
`src/login_item/` and `src/bin/aloud.rs`'s `command_tests`; those were reverted).

---

## 3. The platform experiment — and what it settled

The owner's `/Applications/Aloud.app` was running throughout. Launching a second copy of the
same bundle id would have contended for the global hotkeys and for the `NSServices` port name
`Aloud`, i.e. it could have broken his working reader. So the platform questions were settled
with a **throwaway probe bundle** instead: ~50 lines of Swift making the same MediaPlayer calls
`src/now_playing/macos.rs` makes, signed with the **same "Aloud Dev" identity**, different bundle
id, `LSUIElement`, and **playing no audio at all**. Observed with `log stream --level debug` on
`mediaremoted` (these lines are Debug level and evict from the in-memory buffer within seconds —
`log show` after the fact loses them, which cost one run).

### 3a. Self-signing — the research's open question, closed

```
mediaremoted: Adding client <MRDMediaRemoteClient …, bundleIdentifier =
              com.andriileso.nowplayingprobe, pid = 41776, entitlements=0>
mediaremoted: [MRDNowPlayingPlayerClient] PlaybackState changed from Unknown to Playing
mediaremoted: [MRDNowPlayingClient] CanBeNowPlaying changed to true
```

`entitlements=0`, accepted. **No** `Ignoring setPlaybackState because application does not
contain entitlement`, and no line matching `ignoring|refus|denied|not permitted` anywhere in the
stream. The `_CodeSignature/CodeEntitlement*` files are absent (`error=-10`) and `mediaremoted`
proceeds anyway. A self-signed bundle is fine.

### 3b. Hand-back — the criterion the research called decisive

```
ActiveNowPlayingClient changed from 【 LOCL (Mac) ❯ com.apple.Music (39472) Music ❯ Music 】
                                 to 【 LOCL (Mac) ❯ com.andriileso.nowplayingprobe (41776) ❯ default 】
                              ...
ActiveNowPlayingClient changed from 【 LOCL (Mac) ❯ com.andriileso.nowplayingprobe (41776) ❯ default 】
                                 to 【 LOCL (Mac) ❯ com.apple.Music (39472) Music ❯ Music 】
```

Setting `.stopped` handed the Now Playing session **straight back to Music.app**. Not dead, not
ownerless, nothing relaunched. Reproduced across four separate runs.

**What this does not prove.** It is the Now Playing *client* arbitration, not a physical key
press — the media key routes to the active Now Playing client, so the inference is short, but it
is an inference. And Music.app was running but not actively playing. Both gaps are exactly what
the owner's script closes, and neither can be closed without his finger.

### 3c. What is actually required to become Now Playing

The research said (LIKELY) "the app must actually be playing". Three controlled runs of the same
probe, which plays no audio in any of them:

| Run | Command handler | `nowPlayingInfo` | Became Now Playing | Handed back |
|---|---|---|---|---|
| A — what Aloud ships | yes | yes | **YES** | YES |
| B | yes | **no** | **YES** | YES |
| C | **no** | yes | **NO** | — |

So: **registering at least one remote command is required; publishing metadata is not; and real
audio output is not involved at all.** `playbackState = .playing` plus a live command handler is
the whole qualification. Aloud publishes the title anyway — it is what Control Centre displays —
but the key does not depend on it. This matches `btrcd` (a macOS app that plays no audio and
exists solely to receive these events) and corrects the research's §1 wording.

### 3d. One thing seen that is *not* Aloud, so it does not get mistaken for it later

The probe's startup log contains a TCC line: `TCCAccessRequest … service=kTCCServiceListenEvent`
returning `auth_value=1` (unknown) with `preflight_unknown=true`. **No dialog appeared and the
process did not block.** It comes from AppKit's own startup in a bundle containing zero Aloud
code, ~100 ms *before* the first MediaPlayer call. It is a preflight query, not a request, and
preflight does not prompt — consistent with the research's §4 measurements, which read
`CGPreflightListenEventAccess=false` from an untrusted process without any prompt. MediaPlayer
did not cause it and Aloud does not depend on it.

### 3e. Housekeeping — something I did that the owner may see on screen

I tried to drive Music.app with `osascript` (which the brief permitted) to make the §3b test use
*actively playing* audio. Music did not answer: `Music got an error: AppleEvent timed out
(-1712)`. That is very likely a pending **Automation** permission dialog ("… wants to control
Music"), which I neither clicked nor pursued — granting permissions is not mine to do. **If
there is a stray "wants to control Music" dialog on screen, it is from that attempt, not from
Aloud, and dismissing it is safe.** I stopped driving Music at that point; the owner's own hand
covers that half of the test better anyway.

---

## 4. Arguments against shipping this

Honest list, written before the gate rather than after.

1. **The decisive evidence is one inference short.** §3b is the arbitration layer, not a key
   press, and against a Music.app that was running but idle. Strong, not conclusive. This is why
   §5 exists.
2. **Aloud will speak *over* the owner's music, not instead of it.** macOS has no
   `AVAudioSession` and therefore no interruption model — taking Now Playing does not pause
   Music. If he leaves Music playing and triggers a read, he gets both at once. That is inherent
   to the platform, not a defect here, and Phase 1's behaviour is the same; but the media key
   makes it more likely to be encountered, because now the same key controls both in turn.
3. **Two triggers can now disagree about *which* app they hit.** While Aloud speaks, F8 is
   Aloud's. The instant the read ends, F8 is Music's again. That is the correct behaviour and
   also a moving target for muscle memory. `Cmd+Shift+P` and the tray item are unaffected and
   always mean Aloud — they remain the reliable controls.
4. **`std::mem::forget` on three handler tokens.** Deliberate (the commands retain their own
   targets; the leak only removes doubt) and bounded at three objects for the process lifetime,
   but it is a leak and it should be named as one.
5. **Control Centre now shows Aloud while it reads.** Not requested, unavoidable — it is the
   same `MPNowPlayingInfoCenter` mechanism. Arguably a feature; the owner should know it happens.
6. **Nothing here is reversible-by-config.** There is no setting to turn the media key off. If
   the gate is marginal rather than clean, the right answer is to drop the commit, not to add a
   toggle nobody asked for.

---

## 5. The owner's test — five minutes, and it decides

Run this before anything merges. **You need to press F8 yourself; I deliberately did not
synthesise it.**

Nothing here installs over `/Applications/Aloud.app`. Step 1 quits your running copy only so the
two do not fight over `Cmd+Shift+R` / `Cmd+Shift+A`; step 8 puts it back exactly as it was.

1. **Quit the Aloud you are running now** — click its menubar icon → *Quit Aloud*. (Nothing is
   uninstalled; `/Applications/Aloud.app` is untouched.)

2. **Launch the test build:**
   ```
   open -n "/Users/andrewleso/Desktop/KnowledgeDatabase/Tracks/Side Projects/aloud-pause-resume/target/Aloud.app"
   ```
   A second Aloud icon appears in the menubar. **Watch for any permission dialog** — there should
   be none. If macOS asks for Accessibility or Input Monitoring, stop: that alone kills the
   feature, and I want to know.

3. **Open Music.app and start playing a track. Press F8.** It should pause. Press F8 again — it
   should resume. This is the baseline; leave the track *playing*.

4. **Trigger an Aloud read** from the new menubar icon → *Read Region*, and drag over a paragraph
   of text. (Expect Aloud and Music to be audible at the same time — see §4.2. That is macOS, not
   a bug.)

5. **While Aloud is speaking, press F8.** Aloud should pause — its tray item should read
   *Resume*. Press F8 again: Aloud resumes. Music should be unaffected either way.

6. **Let the read finish completely. Now press F8. This is the decisive moment.** Write down
   which of these happens:
   - Music pauses (or resumes) — **control came back. Pass.**
   - Nothing at all happens — **fail.**
   - Music.app relaunches, or some other app reacts — **fail.**

7. **Repeat step 6 twice more,** because the answer only counts if it is stable:
   - with Music **paused but still running**, and
   - with Music **quit entirely** (here "nothing happens" is fine — there is nothing to control).

8. **Finish:** quit the test Aloud from its menubar icon, then reopen your normal one from
   `/Applications`. Its settings, voice and speed are untouched.

If anything is ambiguous, `~/Library/Logs/Aloud/aloud.log` has a line per event —
`now playing: Playing` / `Paused` / `Stopped`, and `remote command: TogglePlayPause (paused=…)`
for every key press that actually reached Aloud. A press that produced **no** `remote command:`
line never arrived, which distinguishes "Aloud ignored it" from "Aloud never got it".

---

## 6. Recommendation

**Gate criteria, restated verbatim, with where each stands:**

| Kill criterion | Status |
|---|---|
| Needs an Accessibility prompt ⇒ **dead** | **Clear.** No entitlements, no new TCC symbol, no privacy key, no tap. `MPRemoteCommandCenter` is a registration surface. Step 2 confirms with eyes. |
| Control does not return to Music after a read ⇒ **dead** | **Clear at the arbitration layer** (§3b, four runs). Step 6 is the confirmation. |
| Becoming Now Playing requires holding the key while idle ⇒ **dead** | **Clear.** `.stopped` releases immediately and unconditionally, including on error and panic (§1, §2). Aloud holds the key only while `Player::speak` is running. |

**Ship it — conditional on step 6 coming back "Music pauses/resumes" in all three variants of
step 7.** The evidence available without a human finger is as strong as it can be, and it points
one way. But the decisive criterion is a hand-back the owner has to see, and "the code is
written" is not a reason to lower that bar.

**If step 6 fails**, drop this commit. Phase 1 stands on its own: `Cmd+Shift+P` and the tray item
give real pause/resume, keep working while Music owns the media keys, and cost no platform risk
at all. Nothing in Phase 1 depends on any of this.

---

## 7. Not done here

- **No settings toggle for the media key.** None was asked for, and adding one before the gate
  would be building an escape hatch for a decision not yet made (see §4.6).
- **The pause chord still has no settings-window row** — carried over from Phase 1 §6.1, and
  slightly less pressing now that there is a second trigger.
- **`docs/M4-platform-research-macos.md`'s stale `CGEventTap` line and `src/shortcut.rs`'s
  `MEDIA_KEYS` doc comment are both fixed in this change set** — they were named as outstanding
  in Phase 1 §7, and the research doc asked for them to be corrected alongside any media-key
  work. The denylist itself is byte-for-byte unchanged.
