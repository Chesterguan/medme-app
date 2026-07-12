//! macOS on-device OCR via Apple **Vision** (`VNRecognizeTextRequest`) —
//! offline, no model download, strong Chinese. PRIMARY recognizer on the macOS
//! desktop build; `recognize` falls back to oar-ocr / PP-OCRv5 if Vision yields
//! nothing or errors. Pure Rust via `objc2` (no Swift toolchain / link wiring).
//!
//! Decoding goes through Vision's own `initWithData:` (ImageIO/CIImage under
//! the hood), so it accepts **every Apple-supported format incl. HEIC/HEIF**
//! (iPhone photos) — not just what the Rust `image` crate can decode. Mirrors
//! the iOS `OcrVision.swift` behavior (which decodes via `CGImageSource`).

use crate::OcrOutcome;
use anyhow::{anyhow, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
    VNRequestTextRecognitionLevel,
};

/// Recognize text in raw encoded image bytes (JPEG/PNG/TIFF/HEIC/…) via Apple
/// Vision. Returns lines joined top-to-bottom + mean observation confidence.
pub fn recognize_bytes(image_bytes: &[u8]) -> Result<OcrOutcome> {
    // SAFETY: standard Vision usage — VNImageRequestHandler decodes the NSData
    // with ImageIO, then we run one synchronous text request and read the
    // observations. objc2 ref-counts every object.
    unsafe {
        let data = NSData::with_bytes(image_bytes);
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        // Simplified + Traditional Chinese + English (labels/units often EN).
        let langs = NSArray::from_retained_slice(&[
            NSString::from_str("zh-Hans"),
            NSString::from_str("zh-Hant"),
            NSString::from_str("en-US"),
        ]);
        request.setRecognitionLanguages(&langs);
        request.setUsesLanguageCorrection(true);

        let options = NSDictionary::<VNImageOption, AnyObject>::new();
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );

        // VNRecognizeTextRequest is a subclass of VNRequest; performRequests
        // wants an NSArray<VNRequest>.
        let req: Retained<VNRequest> = Retained::cast_unchecked(request.clone());
        let requests = NSArray::from_retained_slice(&[req]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| anyhow!("Vision performRequests failed: {e:?}"))?;

        let mut lines: Vec<String> = Vec::new();
        let mut conf_sum: f32 = 0.0;
        let mut conf_n: u32 = 0;
        if let Some(results) = request.results() {
            for obs in results.iter() {
                let candidates = obs.topCandidates(1);
                if let Some(text) = candidates.firstObject() {
                    let s = text.string().to_string();
                    if !s.trim().is_empty() {
                        lines.push(s);
                        conf_sum += text.confidence();
                        conf_n += 1;
                    }
                }
            }
        }
        Ok(OcrOutcome {
            text: lines.join("\n"),
            confidence: if conf_n > 0 {
                conf_sum / conf_n as f32
            } else {
                0.0
            },
            backend: crate::OcrBackend::AppleVision,
        })
    }
}
