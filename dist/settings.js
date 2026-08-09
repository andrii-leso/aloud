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
// null when no probe is outstanding. Module-level like `recording` — this
// whole file is reloaded fresh every time the settings window is
// (re)created, so there is nothing to reset on open.
let probeTimer = null;

function startRecording() {
  // Starting a new recording abandons any chord that was just saved and
  // is still waiting on a press to confirm it — clear that probe rather
  // than leaving it to time out on its own 10s later, after the user has
  // already moved on to a different chord. Mirrors clearing on Escape,
  // success, and window close: every way of leaving the "waiting to
  // confirm" state disarms the probe.
  if (probeTimer !== null) {
    clearTimeout(probeTimer);
    probeTimer = null;
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
  clearTimeout(probeTimer);
  // Ten seconds is long enough to reach for a chord and short enough
  // that a forgotten window does not swallow a real hotkey press later.
  probeTimer = setTimeout(async () => {
    probeTimer = null;
    await invoke("end_probe");
    setStatus(
      chordStatus,
      "Aloud never saw that shortcut. Another app is probably using it — " +
        "macOS does not report this, so trying a different one is the only fix.",
      "error"
    );
  }, 10000);
}

window.__TAURI__.event.listen("aloud://probe-fired", () => {
  clearTimeout(probeTimer);
  probeTimer = null;
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
  // start, overwrites a genuine "Confirmed" message with the false
  // "never saw that shortcut" error. Flipping `recording` false here,
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

(async function init() {
  const s = await invoke("get_settings");
  recordBtn.textContent = s.region_shortcut_pretty;
  showSpeed(s.speed);
  const voice = document.querySelector(`input[name="voice"][value="${s.voice}"]`);
  if (voice) voice.checked = true;
})().catch((err) => {
  // Everything on this page is populated by that one call. Without a
  // catch, a failure leaves index.html's hardcoded ⌘⇧R on the button, an
  // unset slider and no voice selected — a settings window quietly
  // describing a state the app is not in.
  setStatus(chordStatus, "Could not load settings: " + String(err), "error");
});
