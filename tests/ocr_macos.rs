// Whole-file gate. `aloud::ocr::macos` does not exist under any other
// cfg (see `src/ocr/mod.rs`), so without this the *test target* fails to
// compile on Windows and `cargo test` never reaches a single test —
// including the platform-neutral ones in the other files.
#![cfg(target_os = "macos")]

use aloud::ocr::{macos::VisionOcr, OcrEngine};
use std::path::Path;

#[test]
fn reads_german_text_with_umlauts() {
    let ocr = VisionOcr::new().expect("helper binary should be found");
    let text = ocr
        .recognise(Path::new("tests/fixtures/german_form.png"))
        .expect("ocr should succeed");
    assert!(text.contains("Antragsteller"), "got: {text}");
    assert!(text.contains("ausgefüllte"), "umlaut lost — got: {text}");
    assert!(text.contains("läuft"), "umlaut lost — got: {text}");
}
