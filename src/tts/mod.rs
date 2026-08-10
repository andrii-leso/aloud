pub mod supertonic_engine;
pub mod timestretch;

/// A block of synthesised mono audio.
#[derive(Debug, Clone)]
pub struct Pcm {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Informational only — nothing reads this. In particular the player's
    /// stall watchdog does not: it polls the sink's queue depth, not any
    /// declared duration. `SupertonicEngine` recomputes it from `samples`
    /// after retiming, so it stays true rather than describing the engine's
    /// pre-stretch 1.0 rendering.
    pub duration_s: f32,
}

/// Swappable synthesis backend. Supertonic today; Kokoro is the
/// licence-clean fallback if OpenRAIL-M ever becomes inconvenient.
pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str, lang: &str, speed: f32) -> anyhow::Result<Pcm>;
}
