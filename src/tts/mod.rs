pub mod supertonic_engine;

/// A block of synthesised mono audio.
#[derive(Debug, Clone)]
pub struct Pcm {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub duration_s: f32,
}

/// Swappable synthesis backend. Supertonic today; Kokoro is the
/// licence-clean fallback if OpenRAIL-M ever becomes inconvenient.
pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str, lang: &str, speed: f32) -> anyhow::Result<Pcm>;
    fn sample_rate(&self) -> u32;
}
