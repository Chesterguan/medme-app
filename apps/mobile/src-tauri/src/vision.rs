//! Apple Vision OCR bridge (iOS only).
//!
//! On iOS the Rust `oar-ocr` path is unusable: it auto-downloads a ~21 MB
//! PP-OCR model into `~/.oar`, but the iOS sandbox has no writable home and
//! ships no model. Instead we call Apple's on-device **Vision** framework
//! (`VNRecognizeTextRequest`) — offline, no download, excellent Chinese.
//!
//! ## How the Swift↔Rust bridge works
//! `gen/apple/Sources/medme-mobile/Vision.swift` exposes two C-ABI symbols via
//! `@_cdecl`. The app target compiles that Swift source and links Rust's
//! `libapp.a`, so these `extern "C"` references resolve against the Swift
//! symbols inside the same binary — no Tauri Swift plugin package, no Podfile
//! entry, no CocoaPods/SPM wiring required. `VNImageRequestHandler.perform` is
//! synchronous, so a plain blocking FFI call is all we need.
//!
//! On any non-iOS target (including the macOS host build used for CI/`cargo
//! build`) this whole module is `#[cfg(target_os = "ios")]`-gated out, and
//! ingest falls back to the unchanged desktop `pipeline::ingest`.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::path::Path;

extern "C" {
    /// Runs `VNRecognizeTextRequest` on the image at `path`. Returns a
    /// malloc'd, NUL-terminated UTF-8 string of the recognized lines (joined
    /// top-to-bottom by `\n`); writes the average observation confidence
    /// (0.0–1.0) into `*out_confidence`. Returns null only on allocation
    /// failure. The caller must free the returned pointer with
    /// `medme_vision_free`.
    fn medme_vision_recognize(path: *const c_char, out_confidence: *mut c_double) -> *mut c_char;
    fn medme_vision_free(ptr: *mut c_char);
}

pub struct VisionText {
    pub text: String,
    /// Average recognition confidence across observations, 0.0–1.0.
    pub confidence: f32,
}

/// Recognize text in an image file using Apple Vision. `path` must point at a
/// readable image inside the app sandbox (the ingest temp file qualifies).
pub fn recognize(path: &Path) -> anyhow::Result<VisionText> {
    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| anyhow::anyhow!("path contains NUL byte"))?;
    let mut confidence: c_double = 0.0;
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call;
    // `confidence` is a valid, writable f64. The returned pointer is either
    // null or a malloc'd C string that we copy then free exactly once.
    unsafe {
        let ptr = medme_vision_recognize(c_path.as_ptr(), &mut confidence as *mut c_double);
        if ptr.is_null() {
            anyhow::bail!("Apple Vision returned a null result");
        }
        let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        medme_vision_free(ptr);
        Ok(VisionText {
            text,
            confidence: confidence as f32,
        })
    }
}
