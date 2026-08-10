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

**The same shortened duration cuts a second time, further down.** Review of the
vendored source found a cut this section originally missed —
`TextToSpeech::call`, lines 731-732:

```rust
let dur = duration[0];
let wav_len = (self.sample_rate as f32 * dur) as usize;
let wav_chunk = &wav[..wav_len.min(wav.len())];
```

`duration` here is the already-divided value, so beyond sizing the latent it
also hard-truncates the rendered waveform to `sample_rate × dur`. Two
independent speed-scaled cuts, not one. It does not change the fix — both are
driven by the same argument, and both stop mattering once the engine is only
ever called at 1.0 — but the mechanism is worse than "the decoder elides".

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
  with what has already been written. The correlation window is 20 ms, chosen to
  exceed one pitch period of M5 (male, ~100-120 Hz, an 8-10 ms period); the 8 ms
  first written was *inside* that period, the textbook condition for SOLA to
  lock onto a sub-period feature and warble at the splice rate on the very voice
  Andrii uses.

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
and watched to fail. It carries the owner's exact sentence as a fixture plus a
second unrelated sentence, and transcribes rendered audio, because there is no
cheaper honest oracle for "was this word actually spoken". Its invariant is
deliberately relative — speed 1.0 is the reference, and a faster rendering of
the same text may not contain less — so the transcriber's own quirks largely
cancel out. It skips loudly if the local Whisper install is absent rather than
passing vacuously.

Two things about it are calibration, not slack, and both were measured:

- **It allows a small token budget** (2 words at 1.5×, 3 at 2.0×) instead of
  demanding exact equality. Whisper substitutes about one word per utterance
  regardless of length — "Aloud" as "allowed"/"allow", "brown" as "round", and
  in one run "dog" as "door" *in the 1.0 reference itself* — and occasionally
  two adjacent words together. Those are substitutions, same token count and
  slot, not the deletions this test exists to catch. Calibrated over seven
  consecutive runs, then confirmed still to catch the bug: re-plumbing `speed`
  into the engine fails it at 3 lost words against a budget of 2.
- **It does not assert sub-second utterances**, though §7 identifies them as the
  worst case, because it cannot: Whisper rendered a *clean* 1.5× "Read Region"
  as "Read Readin" on one run in three. That would flake, and a flaky guard gets
  loosened by the next person. Short-utterance behaviour is covered instead by
  the oracle-free unit tests in §10.

It is `#[ignore]`d. Each case is an ONNX synthesis plus a Whisper run, and in a
plain `cargo test --release` that load lands on `tests/latency_budget.rs`, whose
ratio guard enforces hard constraint 5 and has already been observed at 0.82
against its own 0.70 gate under concurrency. Rather than put pressure on a
load-bearing guard, the expensive check is opt-in:

```
cargo test --release --test speed_preserves_words -- --ignored --nocapture
```

The mechanism itself keeps fast, deterministic, always-on coverage in
`src/tts/timestretch.rs`: retiming ratio at both extremes, pitch preservation at
a male fundamental, the tail flush, and a final-burst check.

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

Full suite: **140 passed, 0 failed, 2 ignored** — the 2 ignored being the
pre-existing absolute-latency check and this document's own ASR test, which is
opt-in for the reason above and was run separately (7 consecutive passes). `cargo fmt --check` clean; the new module is
clippy-clean.

## 10. A second defect in the fix itself: the stretch dropped every tail

Review caught this before merge, and it was the same failure this document
exists to eliminate, two orders of magnitude smaller.

The SOLA loop can only splice while a whole block remains ahead of the read
position, so it stopped with up to one analysis hop of input never emitted —
~70 ms at 1.5×, ~90 ms at 2.0×, ~38 ms at 0.7×. Aloud synthesises one chunk per
sentence, so that is the end of *every sentence*, and a sentence closing on a
short plosive ("…inside it.") loses its final consonant. It would have shipped
as the fix for silent word loss.

Fixed by flushing the remainder after the loop. Because every splice ends in a
verbatim copy, `out` ends exactly on the last input sample consumed, so the
remainder is a seamless continuation and is appended as-is — no splice, no
crossfade, no artifact possible. The cost is that the final few tens of
milliseconds play at 1.0 rather than at `speed`; the same is already true of the
first block. Both are fixed, length-independent.

Pinned by two deterministic tests that need no transcriber:
`the_end_of_the_input_is_never_clipped` (the output must end on the input's
final samples, at 0.7/1.25/1.5/2.0) and `a_final_burst_is_not_swallowed`.

## 11. The residual 2.0× losses are the transcriber — now measured, not asserted

This section previously attributed the leftover 2.0× word errors to Whisper on
the strength of nothing but plausibility. The tail-flush bug above was a
competing in-code explanation sized at *exactly* 90 ms at 2.0×, so it had to be
settled properly.

**Re-measured after the tail fix: the 2.0× numbers did not move at all.**
`heading` stayed 0/2 ("re-region" → "Reregion."), `short` stayed 8/9 ("round
fox"). So the tail flush was a real bug, but not this one.

The discriminating experiment is a control that *cannot* lose content: take one
1.0 rendering and retime it two ways — SOLA, and plain linear resampling, which
only interpolates the same waveform and has no splices at all. Any word a
resampler "loses" was lost by the transcriber, by construction.

| input | SOLA 2.0× | resample 2.0× (lossless control) |
|---|---|---|
| owner's sentence | *verbatim* | Try to rectangle anyone on screen, allow to read the text inside it. |
| quick brown fox | The quick round fox jumps over lazy dog. | A quick damn fox jumps at a lady dog. |
| `Read Region` | Re-readin | We lead him. |

The provably-lossless method scores **worse than the shipped path at every
point**. The audio is intact; Whisper `base` is the limit on heavily compressed
speech. The "leading-consonant loss" reading of "brown" → "round" does not
survive either — the resampler preserves every leading consonant by
construction and still produced "damn fox".

This also bounds what the regression test can honestly assert, and is why it
uses a token budget rather than exact equality: see §9.

## 12. Follow-ups, not done here

- **A/B listen at 0.7× on M5**, engine-native versus stretched. Everything
  measured here is at 1.0 or above. Below 1.0 the mechanism is provably benign
  (`duration /= 0.7` gives the decoder *more* canvas), so pinning the engine to
  1.0 across the whole range is a uniformity choice — one code path — not an
  evidenced one. Recorded as such in `CLAUDE.md` constraint 6.
- The normalizer welds an OCR heading onto the paragraph beneath it, because
  Vision separates them with a single `\n` and the soft-break rule turns any
  single newline into a space — hence `Read Region Drag a rectangle …` as one
  run-on utterance with no pause. It did not cause this bug and is not fixed
  here. It is a real prosody defect and deserves its own change.
- The stall-watchdog margin is thinner than the code comments claimed:
  `LATER_CHUNK_CHARS` is 300 (~18-20 s at 1.0), so the 0.7 speed floor leaves
  1-4 s against a 30 s timeout, not the ~30 s the old ~0.27× figure implied.
  Pre-existing and unchanged by this fix — 0.7 produced an equally long buffer
  before it — but the comments in `player.rs`, `aloud_say.rs` and `README.md`
  now say so.
