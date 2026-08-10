//! `MPRemoteCommandCenter` + `MPNowPlayingInfoCenter` (MediaPlayer,
//! macOS 10.12.2+) — the Apple-sanctioned route to the media key, and the
//! only one that needs no TCC grant. See `mod.rs` for why the event-tap
//! alternative is settled against us.
//!
//! **Threading.** Every call here is made from whatever thread the
//! `Player` happens to be on — usually the synthesis thread, not the main
//! one. That is deliberate and it is not a guess: `objc2`'s
//! header-translator marks main-thread-only classes as `MainThreadOnly`
//! (they then require a `MainThreadMarker` to reach at all), and both
//! `MPNowPlayingInfoCenter` and `MPRemoteCommandCenter` are generated
//! here as plain `NSObject` subclasses. Apple annotates neither as
//! main-actor-isolated. `install_remote_commands`, by contrast, is called
//! once from `setup()`, which *is* the main thread.
//!
//! **Privacy.** The published title is the fixed string "Reading aloud" —
//! never the text being read. `nowPlayingInfo` is surfaced in Control
//! Centre and can be relayed to other devices, so putting whatever
//! happened to be on the user's screen into it would leak the exact
//! content of a fully-local app straight into a system-wide, potentially
//! synced surface. There is no version of that which is acceptable here.

use super::{NowPlaying, PlaybackState, RemoteCommand};
use block2::RcBlock;
use objc2_foundation::{ns_string, NSDictionary, NSString};
use objc2_media_player::{
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingPlaybackState,
    MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};
use std::ptr::NonNull;
use std::sync::Arc;

/// What Control Centre and the Now Playing UI show while Aloud speaks.
/// A constant, never the user's text — see the module's privacy note.
fn now_playing_title() -> &'static NSString {
    ns_string!("Reading aloud")
}

/// The real `NowPlaying`, backed by `MPNowPlayingInfoCenter`.
pub struct MediaRemote;

impl NowPlaying for MediaRemote {
    fn publish(&self, state: PlaybackState) {
        // SAFETY: `defaultCenter` is a no-argument class method returning
        // the process-wide singleton; the two setters below are plain
        // property writes on it. Neither is main-thread-only (see the
        // module doc) and neither takes a pointer we could get wrong.
        let center = unsafe { MPNowPlayingInfoCenter::defaultCenter() };

        // Metadata is (re)published on every non-stopped transition
        // rather than once at startup: `nowPlayingInfo` is what the Now
        // Playing UI renders, and re-setting an identical dictionary is
        // idempotent. On `Stopped` it is deliberately left alone —
        // Apple's documented session release is `playbackState`, and
        // `nowPlayingInfo = nil` is documented only as clearing the
        // metadata dictionary, which is not the same thing.
        if state != PlaybackState::Stopped {
            let info = NSDictionary::from_slices(
                &[unsafe { MPMediaItemPropertyTitle }],
                &[now_playing_title().as_ref()],
            );
            // SAFETY: the dictionary is `NSDictionary<NSString,
            // AnyObject>` with an `NSString` value, which is what the
            // "generic should be of the correct type" obligation asks
            // for. The receiver copies it.
            unsafe { center.setNowPlayingInfo(Some(&info)) };
        }

        let raw = match state {
            PlaybackState::Playing => MPNowPlayingPlaybackState::Playing,
            PlaybackState::Paused => MPNowPlayingPlaybackState::Paused,
            PlaybackState::Stopped => MPNowPlayingPlaybackState::Stopped,
        };
        // SAFETY: as above. This is the load-bearing line of the whole
        // feature in both directions: `.playing` is what makes Aloud the
        // Now Playing app (macOS has no AVAudioSession to infer it from),
        // and `.stopped` is what hands the media key back.
        unsafe { center.setPlaybackState(raw) };
        crate::log_line!("now playing: {state:?}");
    }
}

/// Registers the play / pause / toggle handlers, and disables the
/// commands Aloud cannot honour.
///
/// Call once, from the main thread, at startup. `handler` is invoked by
/// the system on every delivered command; it must not block, since it
/// runs on the OS's own callback thread.
///
/// **Why the disables matter.** A Now Playing app receives the whole
/// transport, not only what it registered — so left at their defaults,
/// next/previous and seek (F9 and F7, and their long-press variants)
/// would be routed to Aloud and silently do nothing while it speaks.
/// That is a regression in the owner's music controls, not a missing
/// feature, and it is exactly the "worse than not having it" outcome
/// this phase is gated against. `isEnabled = false` is documented as
/// *"events for this command are not sent to your app"*.
pub fn install_remote_commands(handler: Arc<dyn Fn(RemoteCommand) + Send + Sync>) {
    // SAFETY: `sharedCommandCenter` is a no-argument class method
    // returning the process-wide singleton. Not main-thread-only (see
    // the module doc), though this is in fact called from `setup()`.
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };

    for (command, which) in [
        (
            unsafe { center.togglePlayPauseCommand() },
            RemoteCommand::TogglePlayPause,
        ),
        (unsafe { center.playCommand() }, RemoteCommand::Play),
        (unsafe { center.pauseCommand() }, RemoteCommand::Pause),
    ] {
        let handler = Arc::clone(&handler);
        let block = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
            handler(which);
            // Always `Success`. The alternatives
            // (`NoActionableNowPlayingItem`, `CommandFailed`) report a
            // failure to the system, and the states in which they would
            // be truthful — nothing speaking — are states in which Aloud
            // has already published `.stopped` and should not be
            // receiving the command at all.
            MPRemoteCommandHandlerStatus::Success
        });
        // SAFETY: the block matches the selector's documented signature
        // exactly, as encoded in the binding's type.
        let token = unsafe { command.addTargetWithHandler(&block) };
        // The command retains its own targets, so dropping this token is
        // sound; it is leaked only to remove all doubt, since there is no
        // point in the process's life at which Aloud unregisters. Three
        // objects, once.
        std::mem::forget(token);
        // SAFETY: a plain BOOL property write.
        unsafe { command.setEnabled(true) };
    }

    for command in [
        unsafe { center.nextTrackCommand() },
        unsafe { center.previousTrackCommand() },
        unsafe { center.seekForwardCommand() },
        unsafe { center.seekBackwardCommand() },
    ] {
        // SAFETY: a plain BOOL property write.
        unsafe { command.setEnabled(false) };
    }

    crate::log_line!("now playing: remote commands installed");
}
