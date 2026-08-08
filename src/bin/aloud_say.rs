use aloud::play::{player::Player, sink::RodioSink};
use aloud::text::{detect::detect_lang, normalize::normalize_ocr};
use aloud::tts::supertonic_engine::SupertonicEngine;
use aloud::vendor::supertonic::{is_valid_lang, AVAILABLE_LANGS};
use anyhow::Result;
use clap::Parser;
use std::io::Read;
use std::sync::Arc;

/// Supertonic's documented speed range. Chunks are capped at ~120 chars;
/// below this floor a single chunk can exceed the player's 30s stall
/// watchdog and abort playback with a spurious "audio device appears
/// stalled" error, so out-of-range values are clamped rather than passed
/// through.
const MIN_SPEED: f32 = 0.7;
const MAX_SPEED: f32 = 2.0;

#[derive(Parser)]
#[command(name = "aloud-say", about = "Speak text with a local neural voice")]
struct Args {
    /// Text to speak. Reads stdin when omitted.
    text: Option<String>,

    /// Voice style: F1-F5 or M1-M5.
    #[arg(long, default_value = "F5")]
    voice: String,

    /// Speech speed, 0.7 to 2.0.
    #[arg(long, default_value_t = 1.0)]
    speed: f32,

    /// Force a language tag instead of detecting it.
    #[arg(long)]
    lang: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let raw = match args.text {
        Some(t) => t,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };

    let text = normalize_ocr(&raw);
    if text.is_empty() {
        eprintln!("[aloud] nothing to say");
        return Ok(());
    }

    if let Some(tag) = &args.lang {
        if !is_valid_lang(tag) {
            anyhow::bail!(
                "[aloud] --lang \"{tag}\" is not a recognised language tag; supported tags: {}",
                AVAILABLE_LANGS.join(", ")
            );
        }
    }

    let lang = args.lang.unwrap_or_else(|| detect_lang(&text));

    let speed = args.speed.clamp(MIN_SPEED, MAX_SPEED);
    if speed != args.speed {
        eprintln!(
            "[aloud] --speed {} is outside the supported range ({MIN_SPEED:.1}-{MAX_SPEED:.1}); clamped to {speed:.1}",
            args.speed
        );
    }

    eprintln!("[aloud] lang={lang} voice={} speed={speed}", args.voice);

    let engine = Arc::new(SupertonicEngine::spawn(&args.voice)?);
    let sink = Arc::new(RodioSink::new()?);
    let player = Player::new(engine, sink);
    player.speak(&text, &lang, speed)?;
    Ok(())
}
