// dist/settings.js
//
// Embedded at COMPILE time by tauri::generate_context!() — editing this
// file requires a rebuild before the change appears. There is no bundler
// and no hot reload.

const { invoke } = window.__TAURI__.core;

const recordBtn = document.getElementById("record");
const chordStatus = document.getElementById("chord-status");
const speed = document.getElementById("speed");
const speedValue = document.getElementById("speed-value");
const speedStatus = document.getElementById("speed-status");
const voiceStatus = document.getElementById("voice-status");
const launchToggle = document.getElementById("launch-at-login");
const launchStatus = document.getElementById("launch-status");
const openLoginItems = document.getElementById("open-login-items");

let recording = false;

function setStatus(el, msg, kind) {
  el.textContent = msg || "";
  el.className = "status" + (kind ? " " + kind : "");
}

// Modifier keys fire keydown while the chord is still being assembled.
// Ignoring them here means the user can hold cmd-shift and the button
// keeps waiting for a real key instead of rejecting on the modifier.
const MODIFIERS = new Set([
  "ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight",
  "AltLeft", "AltRight", "MetaLeft", "MetaRight",
]);

// Handle of the pending liveness-probe timeout (see beginProbe below), or
// null once it has fired or been cleared. Module-level like `recording` —
// this whole file is reloaded fresh every time the settings window is
// (re)created, so there is nothing to reset on open.
let probeTimer = null;

// Whether the Rust-side liveness probe (PROBE_ACTIVE) is currently armed
// from this page's point of view. Kept separate from `probeTimer`: the
// 10s timer now fires and clears itself well before the probe should stop
// listening (see beginProbe — a timeout is a non-event, not a verdict, so
// it leaves PROBE_ACTIVE armed instead of calling end_probe), so
// `probeTimer !== null` alone is no longer a reliable stand-in for "is a
// probe outstanding".
let probeArmed = false;

function startRecording() {
  // Starting a new recording abandons any chord that was just saved and
  // is still waiting on a press to confirm it — clear that probe rather
  // than leaving it armed to swallow a press meant for the new chord.
  // This is also what disarms it on Escape: Escape only reaches the
  // keydown handler below while `recording` is true, and `recording` is
  // only ever set true here, so any Escape path already passed through
  // this same guard on the way in. Window close is handled independently,
  // in Rust, on CloseRequested.
  if (probeArmed) {
    clearTimeout(probeTimer);
    probeTimer = null;
    probeArmed = false;
    invoke("end_probe");
  }
  recording = true;
  recordBtn.classList.add("recording");
  recordBtn.textContent = "Press a shortcut…";
  setStatus(chordStatus, "Esc to cancel.", null);
}

async function stopRecording(restoreLabel) {
  recording = false;
  recordBtn.classList.remove("recording");
  if (restoreLabel) recordBtn.textContent = restoreLabel;
}

// Liveness confirmation ("press it now to confirm it works"). macOS
// registers hotkeys non-exclusively — register() succeeds even for a
// chord another app already owns, and the press is then silently
// shadowed with no API to detect it. Asking the user to press the chord
// and watching for a real ShortcutState::Pressed to arrive is the only
// honest confirmation available.
async function beginProbe() {
  await invoke("begin_probe");
  probeArmed = true;
  clearTimeout(probeTimer);
  // Ten seconds is enough to reach for a chord without "Saved. Press it
  // now…" sitting on screen forever. It is NOT a verdict: macOS registers
  // hotkeys non-exclusively, so a chord already owned by another app still
  // registers with Ok(()) and the press is silently shadowed afterwards —
  // there is no API that reports that contention (see
  // docs/M4-platform-research-macos.md §3, "Detecting 'that chord is
  // taken'"). All this timeout actually knows is that no press has
  // arrived *yet* — and the far more likely reason, given the user is
  // looking at this settings window and not their keyboard, is simply
  // that they have not pressed it. So the message states both
  // explanations, likelier one first, instead of asserting a diagnosis
  // Aloud has no evidence for. Critically, it also leaves PROBE_ACTIVE
  // armed rather than calling end_probe: a press arriving after the 10s
  // mark still confirms the chord and replaces this message (see the
  // probe-fired listener below). The probe is disarmed instead by
  // starting a new recording (startRecording, above) or by closing the
  // window (Rust-side, on CloseRequested) — never left armed past either
  // of those.
  probeTimer = setTimeout(() => {
    probeTimer = null;
    setStatus(
      chordStatus,
      "Aloud has not seen that shortcut yet. Press it now to confirm — " +
        "if you already did, another app is probably using it (macOS " +
        "does not report this, so a different chord is the only fix).",
      null
    );
  }, 10000);
}

window.__TAURI__.event.listen("aloud://probe-fired", () => {
  clearTimeout(probeTimer);
  probeTimer = null;
  probeArmed = false;
  setStatus(chordStatus, "Confirmed — that shortcut works.", "ok");
});

recordBtn.addEventListener("click", () => {
  if (!recording) startRecording();
});

window.addEventListener("keydown", async (e) => {
  if (!recording) return;

  // WebKit hands the page key equivalents before the app menu and does
  // not re-send what we consume, so this genuinely suppresses cmd-W and
  // cmd-Q while recording. It cannot suppress true system chords
  // (cmd-Space, cmd-Tab, cmd-shift-3/4/5) — those never reach us at all,
  // which is why the denylist lives in Rust.
  e.preventDefault();
  e.stopPropagation();

  if (e.code === "Escape") {
    const current = await invoke("get_settings");
    stopRecording(current.region_shortcut_pretty);
    setStatus(chordStatus, "", null);
    return;
  }

  if (MODIFIERS.has(e.code)) return;

  // Dead keys report key === "Dead", but the chord model is built from
  // `code` exclusively (never `key`) for exactly this reason — code
  // stays meaningful regardless of dead-key state or active layout, and
  // there is no key-based display logic here to guard: the recorded
  // label comes back from `set_shortcut`'s Rust-side `pretty` string,
  // not from anything computed in this file.
  const chord = {
    code: e.code,
    meta: e.metaKey,
    ctrl: e.ctrlKey,
    alt: e.altKey,
    shift: e.shiftKey,
  };

  // Re-entrancy guard: a physically held key fires OS key-repeat, which
  // means keydown for the same chord can arrive again before this async
  // set_shortcut/beginProbe round trip has resolved. set_shortcut itself
  // is idempotent, so a duplicate call there is harmless — but a second
  // concurrent beginProbe() would arm a second, independent 10s timer,
  // and only the most recently assigned `probeTimer` handle ever gets
  // cleared (by a real confirmation or by starting a new recording). The
  // orphaned first timer keeps running regardless and, ~10s after ITS
  // start, overwrites a genuine "Confirmed" message with the stale
  // "has not seen that shortcut yet" status. Flipping `recording` false here,
  // before the await, closes that window: a repeat keydown sees
  // `recording === false` at the top of this handler and returns
  // immediately, never re-entering this block. `stopRecording()` below
  // already sets it false on the success path; explicitly restoring it
  // on failure keeps the existing "stay in recording mode after a
  // rejected chord" behaviour intact.
  recording = false;

  try {
    const pretty = await invoke("set_shortcut", { chord });
    stopRecording(pretty);
    setStatus(chordStatus, "Saved. Press it now to confirm it works.", null);
    beginProbe();
  } catch (err) {
    // Stay in recording mode so the user can immediately try another
    // chord rather than clicking the button again.
    recording = true;
    setStatus(chordStatus, String(err), "error");
  }
});

document.getElementById("open-services").addEventListener("click", () => {
  invoke("open_services_settings");
});

// The voice the user most recently asked for. A swap takes ~1.4s on a
// background thread, so two quick clicks put two swaps in flight and the
// completion events can arrive in either order; anything not matching the
// latest request is a stale result and must not overwrite the status.
let lastVoiceRequest = null;

// The real outcome of a voice change — set_voice returning Ok() only means
// the choice was saved and the rebuild started. The rebuild itself can
// still fail (a partial ~/.cache/supertonic3 is enough), which used to
// leave the page showing green "Ready." over an engine that never changed.
window.__TAURI__.event.listen("aloud://voice-swapped", (e) => {
  const { voice, error } = e.payload;
  if (voice !== lastVoiceRequest) return;
  if (error) setStatus(voiceStatus, error, "error");
  else setStatus(voiceStatus, "Ready.", "ok");
});

for (const radio of document.querySelectorAll('input[name="voice"]')) {
  radio.addEventListener("change", async () => {
    lastVoiceRequest = radio.value;
    setStatus(voiceStatus, "Switching voice…", null);
    try {
      await invoke("set_voice", { voice: radio.value });
      // Deliberately no "Ready." here: the engine rebuild is still
      // running. The aloud://voice-swapped listener above reports what
      // actually happened.
    } catch (err) {
      lastVoiceRequest = null;
      setStatus(voiceStatus, String(err), "error");
    }
  });
}

function showSpeed(v) {
  speed.value = v;
  speedValue.textContent = Number(v).toFixed(2) + "×";
}

speed.addEventListener("input", () => {
  speedValue.textContent = Number(speed.value).toFixed(2) + "×";
});
speed.addEventListener("change", async () => {
  try {
    const applied = await invoke("set_speed", { speed: Number(speed.value) });
    showSpeed(applied);
    setStatus(speedStatus, "", null);
  } catch (err) {
    // Without this the slider sits at a value nothing was ever saved at,
    // silently. Say what failed, then put the control back to what is
    // actually stored.
    setStatus(speedStatus, String(err), "error");
    const s = await invoke("get_settings").catch(() => null);
    if (s) showSpeed(s.speed);
  }
});

// Renders the launch-at-login controls from a LoginItemView, which the
// Rust side always builds from the OS's LIVE SMAppService status — never
// from the saved `launch_at_login` bool. macOS does not tell Aloud when
// the user switches it off in System Settings > General > Login Items, so
// the only way this checkbox can be trusted is to ask the system every
// time the window loads and after every change.
//
// `note` is present exactly for the states the user has to resolve
// themselves (approval withheld, item not found). Those are also the only
// states where the Login Items button is worth showing: registering again
// from here cannot grant consent, so the button is the actual fix.
// `unsupported` is not an OS status: it means this build has no
// launch-at-login mechanism on this platform (Windows today — see
// src/login_item/windows.rs). It carries a note like the states the user
// must resolve, but unlike those there is nothing the user can do and no
// pane to send them to, so the checkbox is disabled and the button stays
// hidden. Leaving the toggle live would offer to turn on something that
// can only fail.
function showLoginItem(view) {
  const unsupported = view.status === "unsupported";
  launchToggle.checked = view.on;
  launchToggle.disabled = unsupported;
  if (view.note) setStatus(launchStatus, view.note, "error");
  else if (view.on) setStatus(launchStatus, "Aloud will start at login.", "ok");
  else setStatus(launchStatus, "", null);
  openLoginItems.hidden = unsupported || !view.note;
}

// True while a set_launch_at_login round trip is in flight. Registering
// can make macOS post its own "added to Login Items" notification, which
// is enough to bounce focus away and back — and the refresh below fires
// on that. Without this guard that refresh can read the status mid-change
// and paint a stale answer over the real result.
let loginItemBusy = false;

launchToggle.addEventListener("change", async () => {
  const wanted = launchToggle.checked;
  setStatus(launchStatus, wanted ? "Registering…" : "Removing…", null);
  loginItemBusy = true;
  try {
    showLoginItem(await invoke("set_launch_at_login", { enabled: wanted }));
  } catch (err) {
    // The request failed, so nothing was saved. Report that, then put the
    // checkbox back to what is actually true — leaving it showing the
    // request would be the exact "toggle lies about system state" defect
    // this section is built to avoid.
    const view = await invoke("get_login_item_status").catch(() => null);
    if (view) {
      setStatus(launchStatus, String(err), "error");
      launchToggle.checked = view.on;
      // Same rule as showLoginItem: this button is only the fix for the
      // states macOS says the user has to resolve. Showing it for, say, a
      // failed disk write points them at a pane that cannot help.
      openLoginItems.hidden = !view.note;
    } else {
      // Even the read-back failed, so Aloud does not know the system
      // state. Return the checkbox to what it showed before the click and
      // say plainly that it is unconfirmed — the alternative is leaving a
      // checkbox asserting something nothing has verified.
      launchToggle.checked = !wanted;
      setStatus(
        launchStatus,
        String(err) +
          " — and Aloud could not read back the current state. Check " +
          "System Settings → General → Login Items.",
        "error"
      );
      openLoginItems.hidden = false;
    }
  } finally {
    loginItemBusy = false;
  }
});

// Re-read whenever the window comes back to the front. Reading only on
// load is not enough: this window can sit open while the user changes the
// login item in System Settings — the button below sends them there — and
// macOS never notifies Aloud, so the checkbox would keep showing the
// state as of whenever the window happened to open.
async function refreshLoginItem() {
  if (loginItemBusy) return;
  const view = await invoke("get_login_item_status").catch(() => null);
  if (view) showLoginItem(view);
}

// Both, deliberately: `focus` covers returning from another app, and
// `visibilitychange` covers the window being re-shown after being hidden,
// which need not move DOM focus. A duplicate refresh costs one cheap read.
window.addEventListener("focus", refreshLoginItem);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshLoginItem();
});

openLoginItems.addEventListener("click", () => {
  invoke("open_login_items_settings");
});

(async function init() {
  const s = await invoke("get_settings");
  recordBtn.textContent = s.region_shortcut_pretty;
  showSpeed(s.speed);
  const voice = document.querySelector(`input[name="voice"][value="${s.voice}"]`);
  if (voice) voice.checked = true;
  // Deliberately a second call, not a field on get_settings: this one
  // reads live OS state, while get_settings reads what Aloud saved.
  showLoginItem(await invoke("get_login_item_status"));
})().catch((err) => {
  // Everything on this page is populated by that one call. Without a
  // catch, a failure leaves index.html's hardcoded ⌘⇧R on the button, an
  // unset slider and no voice selected — a settings window quietly
  // describing a state the app is not in.
  setStatus(chordStatus, "Could not load settings: " + String(err), "error");
});
