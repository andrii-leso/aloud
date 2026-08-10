# Words dropped mid-sentence — diagnosis, 2026-08-10

Andrii captured the "Read Region" section of Aloud's own settings window and heard

> Drag a rectangle anywhere … reads the text inside it.

`on screen; Aloud` was missing. No error, no notification, nothing in the log.

**Root cause: the speech-speed setting.** Not OCR, not the normalizer, not the
chunker. Supertonic's `speed` argument is not a playback control — it divides
the duration predictor's output and that shortened number sizes the latent the
decoder renders into, so above ~1.1× the decoder is given less room than it
asked for and elides whatever no longer fits. Andrii reads at **1.5×**.

---

## 1. Method

The standing instruction for this class of bug is to capture the raw OCR helper
output before trusting or blaming any downstream layer. That is what settled it,
and it exonerated three layers in one step.

## 2. The log already contained the answer

`~/Library/Logs/Aloud/aloud.log`, the failing run:

```
[1786352673] hotkey: HotKey { mods: Modifiers(SHIFT | SUPER), key: KeyR, ... } pressed
[1786352677] capture: output file present, 20903 bytes
[1786352677] ocr: recognised text length=80 chars
[1786352677] region flow: detected language=en, normalized length=80 chars
[1786352677] region flow: speak started
[1786352682] region flow: speak finished
[1786352682] read_region: completed, spoke
```

`Read Region` (11) + newline + the sentence (68) = **80**. OCR returned 80 and
the normalizer returned 80 — the full text was intact on both sides of
normalization. Whatever lost the words was downstream of both.

Two lines earlier in the same session, the cause was already on the record:

```
[1786352561] voice: switched to M5
[1786352566] speed: now 1.5
```

Every read before that timestamp was at 1.0 and was fine.

## 3. Reproduction

The settings section was re-rendered headlessly from `dist/index.html`'s own
markup and CSS (`<h2>Read Region</h2>` + `<p class="hint">…`) at 2× scale, and
the fixture fed to the real helper.

```
$ ./target/aloud-ocr repro.png | od -c
0000000    R   e   a   d       R   e   g   i   o   n  \n   D   r   a   g
0000020        a       r   e   c   t   a   n   g   l   e       a   n   y
0000040    w   h   e   r   e       o   n       s   c   r   e   e   n   ;
0000060        A   l   o   u   d       r   e   a   d   s       t   h   e
0000100        t   e   x   t       i   n   s   i   d   e       i   t   .
0000121
```

80 characters, matching the owner's real run exactly. **OCR is innocent.**

## 4. Every text stage preserves the string

Instrumented run of the actual pipeline functions:

```
=== 1. RAW OCR (80 chars) ===
"Read Region\nDrag a rectangle anywhere on screen; Aloud reads the text inside it."
=== 2. fix_confusions (80 chars) ===
"Read Region\nDrag a rectangle anywhere on screen; Aloud reads the text inside it."
=== 3. normalize_ocr (80 chars) ===
"Read Region Drag a rectangle anywhere on screen; Aloud reads the text inside it."
=== 4. detect_lang ===  "en"
=== 5. our split_sentences -> 1 chunk(s) ===
  [0] (80 chars) "Read Region Drag a rectangle anywhere on screen; Aloud reads the text inside it."
=== 6. vendor chunk_text + preprocess_text ===
  our chunk [0] -> 1 vendor chunk(s)
    [0.0] (80 chars) "Read Region Drag a rectangle anywhere on screen; Aloud reads the text inside it."
        preprocess -> (89 chars) "<en>Read Region Drag a rectangle anywhere on screen; Aloud reads the text inside it.</en>"
```

The text arrives at the model complete. **The semicolon hypothesis is dead** —
`;` is never a boundary in either splitter, and the 80-char chunk is under both
the 120-char first-chunk cap and the vendored 300-char cap, so no splitting runs
at all. `normalize.rs`, `chunk.rs` and the digit-block/quote behaviours in the
carry-forward doc are all uninvolved.

## 5. The loss is in synthesis, and speed is the variable

Rendered audio transcribed with local Whisper (`base`). Same text, same voice
(M5), only `speed` changed:

| speed | transcript |
|---|---|
| 1.00 | Drag a rectangle anywhere on screen, allowed reads the text inside it. |
| 1.10 | Drag a rectangle anywhere on screen, allowed reads the text inside it. |
| 1.20 | Drag a rectangle anywhere on screen, allowed reads the text inside it. |
| 1.30 | Drag a rectangle anywhere on screen allowed reads the text inside it. |
| 1.40 | Drag a rectangle anywhere on screen, outreads the text **side** it. |
| **1.50** | Drag a rectangle anywhere on **Sallowed** reads the text. |
| 2.00 | Tragger it-tangle our loud reads tool. |

(Whisper hears "Aloud" as "allowed" at every speed, including 1.0 — that is the
transcriber, not the synthesiser.)

The 1.5 row is the owner's report: material gone from the middle, the tail
mangled. The control that proves it is not inherent to fast audio — synthesise
at 1.0, then compress the *rendered waveform* to the identical 3.248 s:

| | transcript |
|---|---|
| engine at 1.5 | Drag a rectangle anywhere on screen reads the text in. |
| 1.0 rendered, then retimed to 1.5 | Drag a rectangle anywhere on screen, allowed reads the text inside it. |

Same duration, same speed to the ear, all the words back.

## 6. Mechanism

`src/vendor/supertonic/mod.rs`, in `_infer`:

```rust
// Predict duration
let duration_view = dp_outputs["duration"].try_extract_tensor::<f32>()?;
let mut duration: Vec<f32> = duration_view.iter().copied().collect();

// Apply speed factor to duration
for dur in duration.iter_mut() {
    *dur /= speed;
}
```

That `duration` then sizes the noisy latent (`sample_noisy_latent`) the
flow-matching decoder fills. The duration predictor has just said how long this
utterance needs to be; dividing by 1.5 hands the decoder 66 % of that. It does
not speak faster — it runs out of canvas and leaves phonemes, then whole words,
out. Silently: there is no error path for "did not fit".

## 7. Other inputs are affected — this was never about one sentence

Six texts, word recall against the reference (before the fix):

| text | 1.00 | 1.25 | 1.50 | 2.00 |
|---|---|---|---|---|
| owner's sentence | 11/12 | 11/12 | **7/12** | **3/12** |
| `Read Region` | 2/2 | **1/2** | **1/2** | **0/2** |
| quick brown fox | 9/9 | 9/9 | **6/9** | **2/9** |
| comma-rich | 13/13 | 13/13 | 13/13 | **8/13** |
| long paragraph | 28/28 | 28/28 | **24/28** | **12/28** |
| plain sentence | 11/11 | 11/11 | 11/11 | **5/11** |

Every speed above ~1.1 loses content on some input. **Short utterances are hit
hardest** — a two-word heading degrades at 1.25, where a long paragraph still
survives — because a short utterance has no slack to absorb the shortfall.
Repeated trials confirm it (synthesis is stochastic, so single samples would not
have been evidence):

```
owner (12 words)              short (9 words)
 1.00: [11, 11, 11]            1.00: [9, 9, 9]
 1.10: [11, 11, 11]            1.10: [9, 9, 9]
 1.20: [11, 10, 10]            1.20: [8, 8, 9]
 1.30: [11,  9, 11]            1.30: [8, 8, 8]
 1.40: [ 9, 10, 10]            1.40: [7, 7, 7]
 1.50: [ 9,  7,  7]            1.50: [7, 4, 3]
```

There is no safe ceiling worth shipping: only 1.0 is reliable for all inputs.

## 8. The fix

**This addresses the cause, not the symptom.** The cause is asking the decoder
to render into less time than its own duration predictor requested. So Aloud
stops doing that, and moves speed to where it belongs — the rendered audio.

- `src/tts/supertonic_engine.rs` — a new `ENGINE_SPEED: f32 = 1.0` constant is
  what reaches `TextToSpeech::call`. The caller's speed is no longer plumbed
  through to the model.
- `src/tts/timestretch.rs` (new) — `time_stretch()` retimes the rendered PCM by
  the requested factor using SOLA (synchronised overlap-add): the waveform is
  copied out in overlapping segments, advancing through input and output at
  different rates, and each splice slides ±10 ms to wherever it best correlates
  with what has already been written.

Aligning splices to the waveform's own periodicity is what preserves pitch.
Plain resampling would have been three lines and is what the control in §5 used,
but it raises a 1.5× male voice by seven semitones. The unit test that pins this
asserts a 200 Hz tone is still 200 Hz after a 1.5× change — a resampler gives
300 Hz.

`src/vendor/` was not touched. The fix sits at Aloud's own engine seam, which is
where the constraint "everything engine-specific lives behind a seam" says it
belongs.

Cost: ~10 ms of CPU per second of audio, after synthesis, which is three orders
of magnitude slower. The chunker is untouched and the latency ratio guard
(`tests/latency_budget.rs`) still passes.

## 9. Verification

`tests/speed_preserves_words.rs` is the regression test, written before the fix
and watched to fail. It carries the owner's exact sentence as a fixture and
transcribes rendered audio, because there is no cheaper honest oracle for "was
this word actually spoken". Its invariant is deliberately relative — speed 1.0
is the reference, and a faster rendering of the same text may not contain less —
so the transcriber's own quirks cancel out. It skips loudly if the local Whisper
install is absent rather than passing vacuously.

Before:

```
speed 1.5 dropped words that speed 1.0 spoke (trial 0): missing ["inside", "it"]
reference: ["drag","a","rectangle","anywhere","on","screen","allowed","reads","the","text","inside","it"]
     fast: ["drag","a","rectangle","anywhere","on","screen","allowed","reads","the","text"]
```

After:

```
  reference_1p0: Drag a rectangle anywhere on screen, allowed reads the text inside it.
  fast_1p5_t0:   Drag a rectangle anywhere on screen, allowed reads the text inside it.
  fast_1p5_t1:   Drag a rectangle anywhere on screen, allowed reads the text inside it.
test raising_the_speed_does_not_drop_words ... ok
```

Word recall across the same six texts, before → after:

| text | 1.00 | 1.25 | 1.50 | 2.00 |
|---|---|---|---|---|
| owner's sentence | 11→11/12 | 11→11/12 | 7→**11**/12 | 3→**11**/12 |
| `Read Region` | 2→2/2 | 1→**2**/2 | 1→**2**/2 | 0→**1**/2 |
| quick brown fox | 9→9/9 | 9→9/9 | 6→**9**/9 | 2→**8**/9 |
| comma-rich | 13→13/13 | 13→13/13 | 13→13/13 | 8→**13**/13 |
| long paragraph | 28→28/28 | 28→28/28 | 24→**28**/28 | 12→**28**/28 |
| plain sentence | 11→11/11 | 11→11/11 | 11→11/11 | 5→**11**/11 |

Full suite: **139 passed, 0 failed, 1 ignored** (the pre-existing `#[ignore]`d
absolute-latency check). `cargo fmt --check` clean; the new module is
clippy-clean.

## 10. Follow-ups, not done here

- At 2.0× the two shortest texts still lose a little to the transcriber
  (`re-region`, `round fox`). The words are being spoken; Whisper struggles with
  heavily compressed short clips. Worth a listen before treating it as a defect.
- The normalizer welds an OCR heading onto the paragraph beneath it, because
  Vision separates them with a single `\n` and the soft-break rule turns any
  single newline into a space — hence `Read Region Drag a rectangle …` as one
  run-on utterance with no pause. It did not cause this bug and is not fixed
  here. It is a real prosody defect and deserves its own change.
