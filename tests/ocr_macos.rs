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
