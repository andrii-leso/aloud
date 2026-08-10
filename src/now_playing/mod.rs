//! Publishing playback state to the OS, so the **physical play/pause
//! media key** (F8 on this MacBook's keyboard) reaches Aloud.
//!
//! This is Phase 2 of pause/resume. Phase 1 built the actual pause
//! (`src/play/`), a tray item and an ordinary `Cmd+Shift+P` chord; this
//! adds a *second trigger* for the same `toggle_pause` and no new
//! playback machinery at all.
//!
//! **The route is `MPRemoteCommandCenter`, never a `CGEventTap`**, and
//! that is measured rather than preferred: on this machine an *active*
//! tap masking `NX_SYSDEFINED` from an untrusted process returns NULL,
//! and a listen-only one is created but arrives `enabled=false` without
//! Input Monitoring. `global-hotkey`'s media-key path uses the active
//! option and upstream confirms it prompts for Accessibility. Aloud must
//! never require an Accessibility grant, so the tap is not a route to
//! this feature — see `docs/media-key-control-research.md` §4, and note
//! that `src/shortcut.rs`'s `MEDIA_KEYS` denylist is unaffected by any
//! of this: it guards the *hotkey rebinding* path, which this feature
//! does not go through.
//!
//! The registration/callback surface here touches none of the TCC gates
//! a tap does: no entitlement, no `Info.plist` key, no prompt.
//!
//! ## The shape macOS requires
//!
//! Two halves, and both are mandatory:
//!
//! 1. Register a handler for at least one `MPRemoteCommandCenter`
//!    command. This is the first of the system's documented
//!    Now-Playing-eligibility heuristics (WWDC22 110338).
//! 2. Set `MPNowPlayingInfoCenter.playbackState` on **every** start and
//!    stop. macOS has no `AVAudioSession`, so there is no central media
//!    server to infer state from and the app must report it: Apple's own
//!    Discussion is *"This property only applies to macOS"* and *"You
//!    must set this property every time the app begins or halts
//!    playback."*
//!
//! `.stopped` is the documented way to *release* the session — Apple's
//! `NowPlayable` sample implements `handleNowPlayableSessionEnd()`, whose
//! stated purpose is *"to allow other apps to become the current
//! NowPlayable app"*, as exactly that one line. Releasing is the whole
//! point of this design: Aloud holds the key only while it is actually
//! speaking. Holding it while idle would make the feature worse than not
//! having it.
//!
//! ## Why a seam
//!
//! Same reason as `RegionSelector` / `OcrEngine` / `TtsEngine` /
//! `AudioSink` / `LoginItemService`: the platform call sits behind a
//! trait so `Player` — which is where the start/stop boundary actually
//! is — carries no `#[cfg]` and stays unit-testable with a recording
//! fake (`tests/now_playing.rs`). `Silent` is the default and does
//! nothing, so a build on any other platform, the `aloud-say` CLI, and
//! every existing test all keep their current behaviour.

#[cfg(target_os = "macos")]
pub mod macos;

/// What Aloud tells the OS it is doing.
///
/// Deliberately three states and not a bool: `Paused` keeps the Now
/// Playing session (so the *next* press of the key resumes Aloud rather
/// than falling through to another app), while `Stopped` gives it up.
/// Collapsing pause into stop would mean the key could pause a read and
/// then never resume it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    /// Nothing is being read. This is what hands the media key back.
    Stopped,
}

/// Publishes `PlaybackState` to whatever the platform's Now Playing
/// mechanism is.
///
/// One method, taking the state rather than exposing `begin`/`end`
/// verbs, because the caller (`Player`) derives the value from its own
/// ground truth on every mutation — there is no second copy of the state
/// to drift, in the same way the tray's Pause/Resume label reads
/// `App::is_paused()` rather than tracking its own bool.
pub trait NowPlaying: Send + Sync {
    fn publish(&self, state: PlaybackState);
}

/// Publishes nothing. The default for every `Player`, so opting *in* to
/// touching the OS is an explicit act at the one call site that wants it
/// (`src/bin/aloud.rs`).
pub struct Silent;

impl NowPlaying for Silent {
    fn publish(&self, _state: PlaybackState) {}
}

/// A remote command as delivered by the system.
///
/// The media key sends `TogglePlayPause`; Control Centre's transport
/// buttons and some headsets send the directional `Play`/`Pause`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCommand {
    Play,
    Pause,
    TogglePlayPause,
}

/// Whether `cmd` should flip the current paused state.
///
/// `Play` and `Pause` are directional and therefore **idempotent** —
/// `Play` on an already-playing read does nothing rather than pausing
/// it. That matters for more than tidiness: if the system ever delivered
/// both a directional command and the toggle for one physical key press,
/// routing all three through a blind toggle would flip twice and look
/// like the key did nothing at all. This makes that failure mode
/// unrepresentable for the directional pair.
///
/// `Play` while idle returns `false`: the media key must never *start* a
/// read. There is nothing to resume, and starting one would need a
/// region drag or a selection that the user has not made.
pub fn should_toggle(cmd: RemoteCommand, paused: bool) -> bool {
    match cmd {
        RemoteCommand::Play => paused,
        RemoteCommand::Pause => !paused,
        RemoteCommand::TogglePlayPause => true,
    }
}
