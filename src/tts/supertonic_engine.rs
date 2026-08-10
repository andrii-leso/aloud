use super::timestretch::time_stretch;
use super::{Pcm, TtsEngine};
use crate::vendor::supertonic::{load_text_to_speech, load_voice_style, Style, TextToSpeech};
use crate::{onnx_dir, voice_style_path};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

const TOTAL_STEP: usize = 8;
const SILENCE_DURATION: f32 = 0.3;

/// The only speed Supertonic is ever asked for.
///
/// Its `speed` argument is not a playback control — it divides the duration
/// predictor's output and sizes the decoder's latent from the result, so any
/// value above ~1.1 hands the decoder less room than it asked for and it drops
/// phonemes, and then words, without raising anything. Andrii lost
/// `on screen; Aloud` out of the middle of a sentence this way at 1.5x
/// (2026-08-10). Speed is applied to the rendered audio instead, by
/// `super::timestretch` — see that module and
/// `docs/2026-08-10-text-drop-diagnosis.md`.
///
/// Do not plumb a caller's speed through to `TextToSpeech::call`.
const ENGINE_SPEED: f32 = 1.0;

struct Job {
    text: String,
    lang: String,
    speed: f32,
    reply: Sender<Result<Pcm>>,
}

pub struct SupertonicEngine {
    tx: Mutex<Sender<Job>>,
}

impl SupertonicEngine {
    /// Loads the model on a dedicated thread and keeps it resident.
    /// The ~1.4s load cost is paid once, here.
    pub fn spawn(voice: &str) -> Result<Self> {
        let onnx = onnx_dir()?
            .to_str()
            .ok_or_else(|| anyhow!("model path is not valid UTF-8"))?
            .to_owned();
        let style_path = voice_style_path(voice)?.to_string_lossy().into_owned();

        let (ready_tx, ready_rx) = channel::<Result<u32>>();
        let (job_tx, job_rx) = channel::<Job>();

        std::thread::Builder::new()
            .name("aloud-tts".into())
            .spawn(move || {
                let loaded: Result<(TextToSpeech, Style)> = (|| {
                    let tts = load_text_to_speech(&onnx, false)?;
                    let style = load_voice_style(&[style_path], false)?;
                    Ok((tts, style))
                })();

                let (mut tts, style, rate) = match loaded {
                    Ok((tts, style)) => {
                        let rate = tts.sample_rate as u32;
                        if ready_tx.send(Ok(rate)).is_err() {
                            return;
                        }
                        (tts, style, rate)
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };

                while let Ok(job) = job_rx.recv() {
                    let result = tts
                        .call(
                            &job.text,
                            &job.lang,
                            &style,
                            TOTAL_STEP,
                            ENGINE_SPEED,
                            SILENCE_DURATION,
                        )
                        .map(|(samples, _duration_s)| {
                            let samples = time_stretch(&samples, rate, job.speed);
                            Pcm {
                                // The engine's reported duration describes the
                                // 1.0 rendering, so it is recomputed from what
                                // is actually being handed to the sink.
                                duration_s: samples.len() as f32 / rate as f32,
                                samples,
                                sample_rate: rate,
                            }
                        });
                    let _ = job.reply.send(result);
                }
            })?;

        // Blocks until the model has loaded, surfacing a load error here
        // rather than on the first synthesize() call. The rate itself is
        // only needed inside the worker thread, to stamp each Pcm.
        ready_rx
            .recv()
            .map_err(|_| anyhow!("tts thread died during model load"))??;

        Ok(Self {
            tx: Mutex::new(job_tx),
        })
    }
}

impl TtsEngine for SupertonicEngine {
    fn synthesize(&self, text: &str, lang: &str, speed: f32) -> Result<Pcm> {
        let (reply, rx) = channel();
        self.tx
            .lock()
            .map_err(|_| anyhow!("tts sender poisoned"))?
            .send(Job {
                text: text.to_owned(),
                lang: lang.to_owned(),
                speed,
                reply,
            })
            .map_err(|_| anyhow!("tts thread is gone"))?;
        rx.recv()
            .map_err(|_| anyhow!("tts thread dropped the job"))?
    }
}
