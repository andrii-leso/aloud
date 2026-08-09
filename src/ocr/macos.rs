use super::OcrEngine;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// OCR via Apple's Vision framework, called out to a bundled Swift helper
/// binary (`aloud-ocr`) rather than linked in-process — Vision has no
/// stable Rust binding, and a subprocess keeps the seam identical to the
/// eventual Windows.Media.Ocr implementation (M6).
pub struct VisionOcr {
    helper: PathBuf,
}

impl VisionOcr {
    /// Locates the `aloud-ocr` helper binary.
    ///
    /// Resolution order, first hit wins:
    /// 1. Next to the running executable — where it lives once bundled
    ///    into the `.app` by a later task.
    /// 2. `target/aloud-ocr` under the crate root — where
    ///    `helpers/macos-ocr/build.sh` puts it during development.
    pub fn new() -> Result<Self> {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("aloud-ocr");
                if candidate.is_file() {
                    return Ok(Self { helper: candidate });
                }
            }
        }

        let dev_candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("aloud-ocr");
        if dev_candidate.is_file() {
            return Ok(Self {
                helper: dev_candidate,
            });
        }

        Err(anyhow!(
            "helper binary not found, run helpers/macos-ocr/build.sh"
        ))
    }
}

impl OcrEngine for VisionOcr {
    fn recognise(&self, image_path: &Path) -> Result<String> {
        let output = Command::new(&self.helper).arg(image_path).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            crate::log_line!(
                "ocr: aloud-ocr exited with {}: {}",
                output.status,
                stderr.trim()
            );
            return Err(anyhow!(
                "aloud-ocr exited with {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let text = String::from_utf8(output.stdout)?.trim_end().to_string();
        crate::log_line!("ocr: recognised text length={} chars", text.chars().count());
        Ok(text)
    }
}
