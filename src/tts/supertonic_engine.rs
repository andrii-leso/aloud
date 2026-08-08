use super::{Pcm, TtsEngine};
use crate::vendor::supertonic::{load_text_to_speech, load_voice_style, Style, TextToSpeech};
use crate::{onnx_dir, voice_style_path};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

const TOTAL_STEP: usize = 8;
const SILENCE_DURATION: f32 = 0.3;

struct Job {
    text: String,
    lang: String,
    speed: f32,
    reply: Sender<Result<Pcm>>,
}

pub struct SupertonicEngine {
    tx: Mutex<Sender<Job>>,
    sample_rate: u32,
}

impl SupertonicEngine {
    /// Loads the model on a dedicated thread and keeps it resident.
    /// The ~1.4s load cost is paid once, here.
    pub fn spawn(voice: &str) -> Result<Self> {
        let onnx = onnx_dir()
            .to_str()
            .ok_or_else(|| anyhow!("model path is not valid UTF-8"))?
            .to_owned();
        let style_path = voice_style_path(voice).to_string_lossy().into_owned();

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
                            job.speed,
                            SILENCE_DURATION,
                        )
                        .map(|(samples, duration_s)| Pcm {
                            samples,
                            sample_rate: rate,
                            duration_s,
                        });
                    let _ = job.reply.send(result);
                }
            })?;

        let sample_rate = ready_rx
            .recv()
            .map_err(|_| anyhow!("tts thread died during model load"))??;

        Ok(Self {
            tx: Mutex::new(job_tx),
            sample_rate,
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
        rx.recv().map_err(|_| anyhow!("tts thread dropped the job"))?
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
