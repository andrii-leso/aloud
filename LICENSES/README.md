# Third-party licences

What is in here, what each file covers, and — more importantly — what is still
missing. Hard constraint 3 (`CLAUDE.md`) makes these obligations load-bearing
*before any release*, so an honest list of the gaps is worth more than a folder
that looks complete.

| File | Covers | Applies to |
|---|---|---|
| `supertonic-MIT.txt` | The Supertonic **code**, vendored at `src/vendor/supertonic/` | The compiled binary |
| `supertonic-weights-OpenRAIL-M.txt` | The Supertonic 3 **model weights** | The 385 MB model, fetched at runtime |

## The two Supertonic licences are not the same licence

This is the mistake the project is most likely to make, so it is written down
twice. **Code is MIT. Weights are BigScience Open RAIL-M.** MIT is permissive;
Open RAIL-M is permissive *plus* a set of use restrictions in its Attachment A,
which a downstream distributor has to pass on. Commercial use is permitted and
royalty-free either way — the obligations are paperwork, not a fee.

`supertonic-weights-OpenRAIL-M.txt` is the verbatim text published at
<https://huggingface.co/Supertone/supertonic-3/resolve/main/LICENSE>, fetched
2026-08-11 (15,007 bytes). The model card declares `license: openrail`.

## What is still missing

**An EULA mirroring Attachment A.** Open RAIL-M requires the use restrictions to
be passed on to end users as enforceable terms. Aloud has no EULA. This is
deliberately not drafted by an engineer — it routes to Rektor. Blocks
distribution, not development.

**A generated third-party NOTICE for the compiled binary.** Aloud statically
links a large Rust dependency tree plus ONNX Runtime, and none of those licences
are reproduced anywhere. This is independent of the model question and applies
the moment a build is handed to anyone. `cargo about` generating a NOTICE at
build time is the cheap fix; it is not set up.

**The weights' licence does not currently travel with the weights.** The
download procedure fetches `onnx/` and `voice_styles/` only, so a local model
directory ends up with no `LICENSE` in it. Any procedure that copies the model
should copy this file alongside it.
