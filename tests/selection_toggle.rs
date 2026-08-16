//! ⌃⌘S is a selection-aware play/pause toggle, not a plain "read this".
//!
//! The Service hands Aloud the selected text on **every** invocation, so
//! the delivered text is itself the intent signal: the same text again
//! means "toggle what you are reading", different text means "read this
//! instead". Two layers are covered here — the pure decision
//! (`app::intent::decide_selection`, which needs neither an audio device
//! nor an `AppHandle`), and the `App` machinery that executes it against a
//! real `Player`.
//!
//! The region path (⌘⇧R) is deliberately NOT part of this: a repeat region
//! press stays a silent no-op, and there is a test below that fails if
//! that protection is weakened.

use aloud::app::actions::Outcome;
use aloud::app::intent::{decide_selection, Current, SelectionAction};
use aloud::app::{App, SelectionOutcome};
use aloud::capture::RegionSelector;
use aloud::ocr::OcrEngine;
use aloud::play::player::Player;
use aloud::play::sink::AudioSink;
use aloud::tts::{Pcm, TtsEngine};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------
// The pure decision. No Player, no sink, no threads.
// ---------------------------------------------------------------------

#[test]
fn nothing_in_flight_reads_the_delivered_text() {
    // `None` is also what `App` publishes the instant a read ends — and
    // what `App::stop` publishes immediately, without waiting for the
    // stopped read to unwind. A stale `Some(...)` in either case would
    // turn the next press into a pause of silence; the `App`-level guards
    // for both are `a_finished_read_does_not_leave_text_behind_that_...`
    // and `stop_then_the_same_selection_starts_a_fresh_read_...` below.
    assert_eq!(decide_selection("Alpha one.", None), SelectionAction::Speak);
}

#[test]
fn the_same_selection_delivered_again_toggles_pause() {
    let current = Current::Selection("Alpha one.".to_string());
    assert_eq!(
        decide_selection("Alpha one.", Some(&current)),
        SelectionAction::TogglePause,
        "the same text again is the whole feature: it must pause what is \
         playing, not restart it and not be dropped"
    );
}

#[test]
fn a_different_selection_interrupts_the_read_in_flight() {
    let current = Current::Selection("Alpha one.".to_string());
    assert_eq!(
        decide_selection("Beta one.", Some(&current)),
        SelectionAction::Interrupt,
        "different text is an explicit 'read this instead', which the old \
         silent no-op could not tell apart from a stray repeat press"
    );
}

#[test]
fn a_selection_delivered_during_a_region_read_always_interrupts() {
    assert_eq!(
        decide_selection("Alpha one.", Some(&Current::Region)),
        SelectionAction::Interrupt,
        "a region read is speaking OCR output, not a selection, so a \
         selection press can never be toggling it"
    );
}

#[test]
fn comparison_ignores_whitespace_at_the_ends() {
    // Re-selecting the same passage picks up a trailing space or newline
    // in some apps and not others, and the difference is invisible on
    // screen. See `comparison_key`.
    let current = Current::Selection("Alpha one. Alpha two.".to_string());
    assert_eq!(
        decide_selection("  Alpha one. Alpha two.\n", Some(&current)),
        SelectionAction::TogglePause
    );
}

#[test]
fn comparison_does_not_normalise_interior_whitespace() {
    // Deliberate: interior differences ARE visible in the selection
    // highlight, and collapsing them would let two selections the user
    // can see are different compare equal.
    let current = Current::Selection("Alpha one. Alpha two.".to_string());
    assert_eq!(
        decide_selection("Alpha one.  Alpha two.", Some(&current)),
        SelectionAction::Interrupt
    );
}

// ---------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------

/// Twelve sentences, so a read of it is comfortably still in flight when
/// the second press lands.
const TEXT_A: &str = "Alpha one. Alpha two. Alpha three. Alpha four. Alpha five. Alpha six. \
                      Alpha seven. Alpha eight. Alpha nine. Alpha ten. Alpha eleven. Alpha twelve.";
const TEXT_A_SENTENCES: usize = 12;

const TEXT_B: &str = "Beta one. Beta two. Beta three.";
const TEXT_B_SENTENCES: usize = 3;

/// A delivery macOS will happily hand us — it has non-whitespace in it, so
/// `selection::selection_worth_speaking` accepts it — that the normalizer
/// nonetheless empties: more than one `"\n\n"`-separated block, every
/// block a short digit run (`text::normalize::normalize_ocr`). Two page
/// numbers dragged across a page break look exactly like this.
const UNSPEAKABLE_SELECTION: &str = "2024\n\n2025";

/// Synthesis speed of the fake engine, and playback duration of one buffer
/// in the fake sink. Both small, both slower than instant — a read has to
/// take long enough for a second press to land in the middle of it.
const SYNTH: Duration = Duration::from_millis(40);
const PLAYBACK: Duration = Duration::from_millis(30);

/// Records what it was asked to synthesize, how long it took, and — the
/// property the busy guard exists for — the maximum number of `synthesize`
/// calls ever in flight at once. `Player::speak` is documented
/// single-caller; two concurrent reads would interleave `sink.append()`
/// calls, and this is how that shows up in a test rather than as a garbled
/// noise nobody is listening to.
struct ConcurrencyEngine {
    inflight: AtomicUsize,
    max_inflight: AtomicUsize,
    texts: Mutex<Vec<String>>,
    /// While set, `synthesize` blocks after recording its text. Real
    /// synthesis is 1.5-15s per sentence depending on machine load
    /// (CLAUDE.md constraint 5), and a read parked in there still holds
    /// the busy flag and `current` — which is where several of the races
    /// below actually live. `SYNTH` alone is too short to aim at.
    gate: AtomicBool,
}

impl ConcurrencyEngine {
    fn new() -> Self {
        Self {
            inflight: AtomicUsize::new(0),
            max_inflight: AtomicUsize::new(0),
            texts: Mutex::new(Vec::new()),
            gate: AtomicBool::new(false),
        }
    }

    fn calls(&self) -> usize {
        self.texts.lock().unwrap().len()
    }

    /// Park the next `synthesize` call (and any after it) until `release`.
    fn hold(&self) {
        self.gate.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        self.gate.store(false, Ordering::SeqCst);
    }

    /// How many synthesized chunks mention `word`.
    fn calls_mentioning(&self, word: &str) -> usize {
        self.texts
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.contains(word))
            .count()
    }
}

impl TtsEngine for ConcurrencyEngine {
    fn synthesize(&self, text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_inflight.fetch_max(now, Ordering::SeqCst);
        self.texts.lock().unwrap().push(text.to_string());
        while self.gate.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(SYNTH);
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        Ok(Pcm {
            samples: vec![0.0; 100],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

/// Serial-playback sink modelling rodio's pause semantics: while paused it
/// consumes no samples and its depth is frozen, and `stop()` clears the
/// pause (the `AudioSink::stop` contract). Same shape as the one in
/// `tests/player_pause.rs`.
struct PausableSink {
    state: Mutex<PausableState>,
    appended: AtomicUsize,
    paused: AtomicBool,
}

struct PausableState {
    /// Remaining playback time of each outstanding buffer, oldest first.
    remaining: Vec<Duration>,
    last_tick: Instant,
}

impl PausableSink {
    fn new() -> Self {
        Self {
            state: Mutex::new(PausableState {
                remaining: Vec::new(),
                last_tick: Instant::now(),
            }),
            appended: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
        }
    }

    fn appended(&self) -> usize {
        self.appended.load(Ordering::SeqCst)
    }

    /// Advances playback by the wall time since the last call — unless
    /// paused, in which case the clock is consumed but no audio is.
    fn tick(&self, st: &mut PausableState) {
        let now = Instant::now();
        let mut elapsed = now.duration_since(st.last_tick);
        st.last_tick = now;
        if self.paused.load(Ordering::SeqCst) {
            return;
        }
        while elapsed > Duration::ZERO {
            let Some(head) = st.remaining.first_mut() else {
                break;
            };
            if *head > elapsed {
                *head -= elapsed;
                break;
            }
            elapsed -= *head;
            st.remaining.remove(0);
        }
    }
}

impl AudioSink for PausableSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        st.remaining.push(PLAYBACK);
        self.appended.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn queued(&self) -> usize {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        st.remaining.len()
    }

    fn stop(&self) {
        let mut st = self.state.lock().unwrap();
        st.remaining.clear();
        st.last_tick = Instant::now();
        self.paused.store(false, Ordering::SeqCst);
    }

    fn pause(&self) {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        self.paused.store(true, Ordering::SeqCst);
    }

    fn resume(&self) {
        let mut st = self.state.lock().unwrap();
        st.last_tick = Instant::now();
        self.paused.store(false, Ordering::SeqCst);
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// A selector handing back a real temp file, so the region flow's
/// delete-on-drop has something to delete.
struct FileSelector(PathBuf);
impl RegionSelector for FileSelector {
    fn select(&self) -> anyhow::Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

/// A selector that reports it has been entered and then blocks until
/// released.
///
/// `App::read_region` takes the busy flag and publishes `Current::Region`
/// *before* calling `select()`, so a read parked in here sits in exactly
/// the state a takeover's `stop()` cannot see: it owns the flag, it is
/// named by `current`, and it has not been anywhere near `Player::speak`
/// — which clears the stop flag as its first act.
struct BlockingSelector {
    path: PathBuf,
    gate: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
}
impl RegionSelector for BlockingSelector {
    fn select(&self) -> anyhow::Result<Option<PathBuf>> {
        self.entered.store(true, Ordering::SeqCst);
        while self.gate.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Some(self.path.clone()))
    }
}

/// Neither of these may be reached — used to prove the region path's
/// stray-double-press guard short-circuits before it does any work.
struct UnreachableSelector;
impl RegionSelector for UnreachableSelector {
    fn select(&self) -> anyhow::Result<Option<PathBuf>> {
        panic!("a repeat region press must not reach the selector");
    }
}
struct UnreachableOcr;
impl OcrEngine for UnreachableOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        panic!("a repeat region press must not reach OCR");
    }
}

struct SuccessOcr(&'static str);
impl OcrEngine for SuccessOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        Ok(self.0.to_string())
    }
}

fn make_temp_image(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "aloud-selection-toggle-{name}-{}.png",
        std::process::id()
    ));
    std::fs::write(&path, b"fake png bytes").expect("write temp fixture");
    path
}

// ---------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------

struct Rig {
    app: Arc<App>,
    engine: Arc<ConcurrencyEngine>,
    sink: Arc<PausableSink>,
}

fn rig() -> Rig {
    let engine = Arc::new(ConcurrencyEngine::new());
    let sink = Arc::new(PausableSink::new());
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::clone(&sink) as Arc<dyn AudioSink>,
    );
    Rig {
        app: Arc::new(App::new(player, 1.0)),
        engine,
        sink,
    }
}

/// Blocks until `pred` holds, failing the test rather than hanging if it
/// never does.
fn wait_until(what: &str, pred: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pred() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs `f` on its own thread and fails if it has not returned within
/// `limit`.
///
/// Not decoration: a regression that leaves the sink paused under a new
/// read would block `wait_for_drain` **forever** — a paused sink never
/// drains, and the stall watchdog deliberately does not fire while paused
/// (Phase 1, `StallWatch::observe`). Without this the whole test binary
/// would hang instead of one test failing.
fn with_deadline<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .expect("call did not return in time - the sink was probably left paused")
}

/// Starts a selection read on a background thread and waits until audio
/// has genuinely started — the first buffer has reached the sink, not
/// merely the first `synthesize` call has been entered.
///
/// The distinction matters: reaching the engine is the *silent pre-roll*,
/// which is seconds long on a real machine and in which there is nothing
/// to pause. `a_press_during_the_silent_pre_roll_is_not_taken_as_a_pause`
/// covers that window on purpose; every other test here wants to be past
/// it.
fn start_read(
    app: &Arc<App>,
    sink: &Arc<PausableSink>,
    text: &'static str,
) -> std::thread::JoinHandle<anyhow::Result<SelectionOutcome>> {
    let bg = Arc::clone(app);
    let handle = std::thread::spawn(move || bg.speak_selection(text));
    let s = Arc::clone(sink);
    wait_until("the first buffer to reach the sink", move || {
        s.appended() >= 1
    });
    handle
}

// ---------------------------------------------------------------------
// App: the state machine against a real Player
// ---------------------------------------------------------------------

#[test]
fn the_same_selection_pressed_again_pauses_then_resumes() {
    let Rig { app, engine, sink } = rig();
    let handle = start_read(&app, &sink, TEXT_A);

    assert_eq!(
        app.speak_selection(TEXT_A).unwrap(),
        SelectionOutcome::Toggled { paused: true },
        "the same text while speaking must pause"
    );
    assert!(app.is_paused());
    assert!(
        !handle.is_finished(),
        "pausing must leave the read in flight - it must not be stopped, \
         and it must not be restarted"
    );

    // Nothing may run away while paused: at most the one buffer already
    // in flight lands (the documented one-sentence look-ahead).
    let at_pause = engine.calls();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        engine.calls() <= at_pause + 1,
        "the read kept synthesising while paused: {} -> {}",
        at_pause,
        engine.calls()
    );

    assert_eq!(
        app.speak_selection(TEXT_A).unwrap(),
        SelectionOutcome::Toggled { paused: false },
        "the same text again while paused must resume"
    );
    assert!(!app.is_paused());

    let outcome = handle
        .join()
        .expect("the read thread must not panic")
        .expect("a paused read must not abort");
    assert_eq!(outcome, SelectionOutcome::Spoke { replaced: false });
    assert_eq!(
        engine.calls(),
        TEXT_A_SENTENCES,
        "exactly one synthesis per sentence - resume continues from where \
         it stopped, it never re-reads"
    );
    assert_eq!(sink.appended(), TEXT_A_SENTENCES);
    assert_eq!(engine.max_inflight.load(Ordering::SeqCst), 1);
}

#[test]
fn a_different_selection_stops_the_current_read_and_starts_the_new_one() {
    let Rig { app, engine, sink } = rig();
    let handle = start_read(&app, &sink, TEXT_A);
    wait_until("a second sentence to be under way", {
        let e = Arc::clone(&engine);
        move || e.calls() >= 2
    });

    let outcome = {
        let app = Arc::clone(&app);
        with_deadline(Duration::from_secs(10), move || app.speak_selection(TEXT_B))
    }
    .unwrap();

    assert_eq!(
        outcome,
        SelectionOutcome::Spoke { replaced: true },
        "different text must interrupt the read in flight and speak, not \
         no-op and not queue behind it"
    );

    handle
        .join()
        .expect("the displaced read must not panic")
        .expect("a stopped read returns Ok, it is not an error");

    assert!(
        engine.calls_mentioning("Alpha") < TEXT_A_SENTENCES,
        "the displaced read ran to completion ({} of {} sentences) - it was \
         not actually interrupted",
        engine.calls_mentioning("Alpha"),
        TEXT_A_SENTENCES
    );
    assert_eq!(
        engine.calls_mentioning("Beta"),
        TEXT_B_SENTENCES,
        "the new selection must be read in full"
    );
    assert_eq!(
        engine.max_inflight.load(Ordering::SeqCst),
        1,
        "the interrupt must never produce two concurrent Player::speak \
         calls - that is the contract the busy guard exists for"
    );
}

#[test]
fn a_selection_arriving_during_a_region_read_takes_over() {
    let Rig {
        app,
        engine,
        sink: _,
    } = rig();
    let image = make_temp_image("region-takeover");

    let handle = {
        let app = Arc::clone(&app);
        let selector = FileSelector(image.clone());
        std::thread::spawn(move || app.read_region(&selector, &SuccessOcr(TEXT_A)))
    };
    wait_until("the region read to reach the engine", {
        let e = Arc::clone(&engine);
        move || e.calls() >= 1
    });

    let outcome = {
        let app = Arc::clone(&app);
        with_deadline(Duration::from_secs(10), move || app.speak_selection(TEXT_B))
    }
    .unwrap();

    assert_eq!(
        outcome,
        SelectionOutcome::Spoke { replaced: true },
        "⌃⌘S during a region read must interrupt it and read the selection \
         - and must not deadlock against the busy flag waiting for it"
    );

    let region = handle
        .join()
        .expect("the region read must not panic")
        .expect("a stopped region read is not an error");
    assert_eq!(
        region,
        Some(Outcome::Interrupted),
        "the region read was displaced, so it must not report itself as \
         having completed - `src/bin/aloud.rs` logs 'completed, spoke' and \
         resets the tray status on Outcome::Spoke, while the replacement \
         is already speaking"
    );
    // Without this the test passes against an implementation that merely
    // *waits out* the region read instead of stopping it: `replaced` is
    // computed from the decision, not from what `stop()` did, and TEXT_A
    // finishes well inside the 10s deadline wrapper. This is the
    // region-specific half of what its sibling above asserts.
    assert!(
        engine.calls_mentioning("Alpha") < TEXT_A_SENTENCES,
        "the region read ran to completion ({} of {} sentences) - it was \
         waited out, not interrupted",
        engine.calls_mentioning("Alpha"),
        TEXT_A_SENTENCES
    );
    assert_eq!(engine.calls_mentioning("Beta"), TEXT_B_SENTENCES);
    assert_eq!(engine.max_inflight.load(Ordering::SeqCst), 1);
}

#[test]
fn a_takeover_still_stops_a_read_that_has_not_reached_the_player_yet() {
    // The protocol hole the re-asserted stop in `acquire_within` closes.
    //
    // A read owns the busy flag and is named by `current` from the moment
    // it acquires — but it has not entered `Player::speak` yet, and
    // `speak`'s first act is to clear the stop flag. A takeover deciding
    // Interrupt against it therefore issues a `stop()` that the displaced
    // read wipes on entry, and then blocks on the busy flag while the
    // passage it meant to replace plays out in full. If that passage
    // outlasts TAKEOVER_WAIT the user's selection is dropped entirely.
    //
    // The region path is used to hold the window open deterministically:
    // `read_region` acquires the flag and publishes `current` *before*
    // calling `select()`, so a blocking selector parks a read in exactly
    // that state. Nothing about the hole is region-specific — the
    // selection path has the same window between `acquire_within`
    // returning and `Player::speak` being entered (the player read lock,
    // `detect_lang`'s lazy lingua load, `split_sentences`), it is just
    // milliseconds wide and cannot be aimed at from a test.
    let Rig {
        app,
        engine,
        sink: _,
    } = rig();
    let image = make_temp_image("takeover-before-speak");
    let gate = Arc::new(AtomicBool::new(true));
    let entered = Arc::new(AtomicBool::new(false));

    let handle = {
        let app = Arc::clone(&app);
        let selector = BlockingSelector {
            path: image.clone(),
            gate: Arc::clone(&gate),
            entered: Arc::clone(&entered),
        };
        std::thread::spawn(move || app.read_region(&selector, &SuccessOcr(TEXT_A)))
    };
    wait_until("the region read to own the busy flag", {
        let e = Arc::clone(&entered);
        move || e.load(Ordering::SeqCst)
    });

    let takeover = {
        let app = Arc::clone(&app);
        std::thread::spawn(move || app.speak_selection(TEXT_B))
    };
    // Let the takeover issue its stop and settle into the wait, then let
    // the region read run on into `Player::speak`.
    std::thread::sleep(Duration::from_millis(60));
    gate.store(false, Ordering::SeqCst);

    let outcome = takeover
        .join()
        .expect("the takeover must not panic")
        .unwrap();
    assert_eq!(outcome, SelectionOutcome::Spoke { replaced: true });

    let region = handle.join().expect("no panic").unwrap();
    assert_eq!(
        region,
        Some(Outcome::Interrupted),
        "the stop was wiped by `Player::speak` and the displaced read ran \
         to the end"
    );
    assert!(
        engine.calls_mentioning("Alpha") <= 2,
        "the displaced read synthesised {} of {} sentences - the stop \
         request did not survive its entry into Player::speak",
        engine.calls_mentioning("Alpha"),
        TEXT_A_SENTENCES
    );
    assert_eq!(engine.calls_mentioning("Beta"), TEXT_B_SENTENCES);
    assert_eq!(engine.max_inflight.load(Ordering::SeqCst), 1);
}

#[test]
fn a_repeat_region_press_is_still_a_silent_no_op() {
    // The region path is unchanged on purpose: there is no delivered text
    // to compare, so a second ⌘⇧R cannot be told apart from a stray
    // double-press, and interrupting on it would cut off whatever the user
    // is listening to.
    let Rig {
        app,
        engine,
        sink: _,
    } = rig();
    let image = make_temp_image("region-repeat");

    let handle = {
        let app = Arc::clone(&app);
        let selector = FileSelector(image.clone());
        std::thread::spawn(move || app.read_region(&selector, &SuccessOcr(TEXT_A)))
    };
    wait_until("the region read to reach the engine", {
        let e = Arc::clone(&engine);
        move || e.calls() >= 1
    });

    assert_eq!(
        app.read_region(&UnreachableSelector, &UnreachableOcr)
            .unwrap(),
        None,
        "a second region press while one is in flight must be skipped"
    );

    let first = handle.join().expect("no panic").unwrap();
    assert_eq!(first, Some(Outcome::Spoke));
    assert_eq!(
        engine.calls_mentioning("Alpha"),
        TEXT_A_SENTENCES,
        "the first region read must have run to completion - a repeat press \
         must not have interrupted it"
    );
}

#[test]
fn a_finished_read_does_not_leave_text_behind_that_toggles_instead_of_reading() {
    let Rig { app, engine, sink } = rig();

    assert_eq!(
        app.speak_selection(TEXT_B).unwrap(),
        SelectionOutcome::Spoke { replaced: false }
    );
    assert_eq!(
        app.speak_selection(TEXT_B).unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the first read has finished, so the same text again is a fresh \
         read - a stale 'currently reading' value would pause silence here"
    );

    assert_eq!(engine.calls(), 2 * TEXT_B_SENTENCES);
    assert_eq!(sink.appended(), 2 * TEXT_B_SENTENCES);
    assert!(!app.is_paused());
}

#[test]
fn superseding_a_paused_read_does_not_leave_the_new_one_playing_into_a_paused_sink() {
    // The already-discovered rodio trap: a paused sink swallows whatever
    // is appended next, silently, with a queue depth that never moves.
    let Rig { app, engine, sink } = rig();
    let handle = start_read(&app, &sink, TEXT_A);

    assert_eq!(
        app.speak_selection(TEXT_A).unwrap(),
        SelectionOutcome::Toggled { paused: true }
    );
    assert!(app.is_paused(), "the read must actually be paused");
    let appended_before = sink.appended();

    let outcome = {
        let app = Arc::clone(&app);
        with_deadline(Duration::from_secs(10), move || app.speak_selection(TEXT_B))
    }
    .unwrap();

    assert_eq!(outcome, SelectionOutcome::Spoke { replaced: true });
    assert!(
        !app.is_paused(),
        "the new read inherited the superseded read's pause - it would be \
         silent, forever, with no error anywhere"
    );
    handle.join().expect("no panic").expect("no error");

    assert_eq!(
        engine.calls_mentioning("Beta"),
        TEXT_B_SENTENCES,
        "the new selection must have been synthesized in full"
    );
    assert!(
        sink.appended() >= appended_before + TEXT_B_SENTENCES,
        "the new selection's buffers must have reached the sink"
    );
}

#[test]
fn stop_then_the_same_selection_starts_a_fresh_read_rather_than_toggling() {
    // Stop is asynchronous: it silences the sink at once, but the reading
    // thread does not notice until it leaves `engine.synthesize()` — up
    // to 15s on a loaded machine (CLAUDE.md constraint 5) — and it holds
    // the busy flag and `current` for all of it. If `current` survives
    // the stop, the user's natural next move (tray → Stop, then ⌃⌘S with
    // the same passage still highlighted, to start it over) is decided as
    // TogglePause and swallowed: nothing is read, nothing is reported,
    // and the tray flips to "Resume" for a read that is already dead.
    let Rig { app, engine, sink } = rig();
    let handle = start_read(&app, &sink, TEXT_A);

    // Park the read inside the engine, where a real one spends nearly all
    // of its life, so the window is a real one rather than a lucky
    // microsecond.
    engine.hold();
    wait_until("the read to park inside the engine", {
        let e = Arc::clone(&engine);
        move || e.calls() >= 2
    });

    app.stop(); // the tray's Stop item
    let synthesised_before = engine.calls_mentioning("Alpha");

    let press = {
        let app = Arc::clone(&app);
        std::thread::spawn(move || app.speak_selection(TEXT_A))
    };
    // The decision is taken the instant the press lands — while the
    // stopped read is still unwinding. Let that happen, then let the dead
    // read go.
    std::thread::sleep(Duration::from_millis(60));
    engine.release();

    let outcome = press.join().expect("the press must not panic").unwrap();
    assert_eq!(
        outcome,
        SelectionOutcome::Spoke { replaced: false },
        "⌃⌘S after a Stop must start a fresh read, not toggle pause on a \
         read that has already been silenced"
    );

    handle
        .join()
        .expect("the stopped read must not panic")
        .expect("a stopped read returns Ok, it is not an error");

    assert_eq!(
        engine.calls_mentioning("Alpha") - synthesised_before,
        TEXT_A_SENTENCES,
        "the passage was not actually read again"
    );
    assert!(!app.is_paused(), "nothing should have been left paused");
    assert_eq!(engine.max_inflight.load(Ordering::SeqCst), 1);
}

#[test]
fn a_press_during_the_silent_pre_roll_is_not_taken_as_a_pause() {
    // Time-to-first-audio is seconds, and wholly load-dependent
    // (CLAUDE.md constraint 5). A user who hears nothing and presses ⌃⌘S
    // again is in the pre-roll, where `Player::speak` has already set
    // `speaking` but nothing has reached the sink. Accepting that as a
    // pause parks the read forever: the first buffer lands in a paused
    // sink, `wait_for_drain` sits on a frozen depth, and `StallWatch`
    // deliberately never fires while paused — so the busy flag is held
    // for good and ⌘⇧R goes dead too. Before ⌃⌘S became a toggle, the
    // same double-press was a harmless no-op.
    let Rig { app, engine, sink } = rig();
    engine.hold();

    let bg = Arc::clone(&app);
    let handle = std::thread::spawn(move || bg.speak_selection(TEXT_A));
    wait_until("the read to reach the engine", {
        let e = Arc::clone(&engine);
        move || e.calls() >= 1
    });
    assert_eq!(
        sink.appended(),
        0,
        "the test is not in the pre-roll it means to measure"
    );

    assert_eq!(
        app.speak_selection(TEXT_A).unwrap(),
        SelectionOutcome::Toggled { paused: false },
        "a pause was accepted before a single sample had been produced"
    );
    assert!(
        !app.is_paused(),
        "the sink was paused while empty - the read will never make a sound"
    );

    engine.release();
    let outcome = with_deadline(Duration::from_secs(10), move || {
        handle
            .join()
            .expect("the read must not panic")
            .expect("the read must not error")
    });
    assert_eq!(outcome, SelectionOutcome::Spoke { replaced: false });
    assert_eq!(engine.calls(), TEXT_A_SENTENCES);
    assert_eq!(sink.appended(), TEXT_A_SENTENCES);
}

#[test]
fn a_selection_that_normalizes_to_nothing_leaves_the_read_in_flight_alone() {
    // The premise, asserted rather than assumed: macOS delivers this (it
    // is not whitespace-only, so `selection_worth_speaking` accepts it)
    // and the normalizer empties it. The single-block escape hatch that
    // preserves a bare "42" does not cover a multi-block selection.
    assert!(
        aloud::text::normalize::normalize_ocr(UNSPEAKABLE_SELECTION).is_empty(),
        "the fixture no longer normalizes to nothing, so this test is \
         measuring something else"
    );
    assert!(
        aloud::selection::selection_worth_speaking(Some(UNSPEAKABLE_SELECTION)).is_some(),
        "the Service filter would have refused this delivery, so it could \
         never reach App"
    );

    let Rig { app, engine, sink } = rig();
    let handle = start_read(&app, &sink, TEXT_A);

    let outcome = {
        let app = Arc::clone(&app);
        with_deadline(Duration::from_secs(10), move || {
            app.speak_selection(UNSPEAKABLE_SELECTION)
        })
    }
    .unwrap();

    assert_eq!(
        outcome,
        SelectionOutcome::Empty,
        "a delivery with nothing speakable in it must not be reported as \
         having spoken, let alone as having replaced anything"
    );
    assert!(
        !handle.is_finished(),
        "the read in flight was silenced to make room for a selection that \
         then said nothing at all"
    );

    let first = handle
        .join()
        .expect("no panic")
        .expect("the read must not error");
    assert_eq!(
        first,
        SelectionOutcome::Spoke { replaced: false },
        "the read in flight must have run to its own end"
    );
    assert_eq!(
        engine.calls_mentioning("Alpha"),
        TEXT_A_SENTENCES,
        "the read in flight was cut short"
    );
}
