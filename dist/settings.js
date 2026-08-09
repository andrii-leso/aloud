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

function startRecording() {
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

// Liveness confirmation ("press it now to confirm it works") — built in
// Task 8. Stubbed here so the recorder works end to end without it.
async function beginProbe() {}

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

  // Dead keys report key === "Dead"; code is still meaningful, so this
  // only guards the display label below.
  const chord = {
    code: e.code,
    meta: e.metaKey,
    ctrl: e.ctrlKey,
    alt: e.altKey,
    shift: e.shiftKey,
  };

  try {
    const pretty = await invoke("set_shortcut", { chord });
    stopRecording(pretty);
    setStatus(chordStatus, "Saved. Press it now to confirm it works.", null);
    beginProbe();  // Task 8
  } catch (err) {
    // Stay in recording mode so the user can immediately try another
    // chord rather than clicking the button again.
    setStatus(chordStatus, String(err), "error");
  }
});

document.getElementById("open-services").addEventListener("click", () => {
  invoke("open_services_settings");
});

for (const radio of document.querySelectorAll('input[name="voice"]')) {
  radio.addEventListener("change", async () => {
    setStatus(voiceStatus, "Switching voice…", null);
    try {
      await invoke("set_voice", { voice: radio.value });
      setStatus(voiceStatus, "Ready.", "ok");
    } catch (err) {
      setStatus(voiceStatus, String(err), "error");
    }
  });
}

speed.addEventListener("input", () => {
  speedValue.textContent = Number(speed.value).toFixed(2) + "×";
});
speed.addEventListener("change", async () => {
  const applied = await invoke("set_speed", { speed: Number(speed.value) });
  speed.value = applied;
  speedValue.textContent = Number(applied).toFixed(2) + "×";
});

(async function init() {
  const s = await invoke("get_settings");
  recordBtn.textContent = s.region_shortcut_pretty;
  speed.value = s.speed;
  speedValue.textContent = Number(s.speed).toFixed(2) + "×";
  const voice = document.querySelector(`input[name="voice"][value="${s.voice}"]`);
  if (voice) voice.checked = true;
})();
