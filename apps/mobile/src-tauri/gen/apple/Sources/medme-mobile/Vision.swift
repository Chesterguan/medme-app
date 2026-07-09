// Apple Vision OCR bridge for MedMe iOS.
//
// The Rust `oar-ocr` path cannot run in the iOS sandbox (it downloads a ~21 MB
// PP-OCR model into ~/.oar, which is not writable and ships no model). Instead
// this file runs Apple's on-device **Vision** framework (VNRecognizeTextRequest)
// — offline, no download, strong Chinese support.
//
// It is exposed to Rust as two plain C-ABI symbols via `@_cdecl`. The iOS app
// target compiles this Swift file and links Rust's `libapp.a`, so the Rust
// `extern "C"` declarations in `src/vision.rs` resolve directly against these
// symbols inside the same binary — no Tauri Swift plugin package, no CocoaPods
// or SPM wiring needed. `VNImageRequestHandler.perform` is synchronous, so the
// call is a simple blocking FFI (Rust owns the returned buffer and frees it via
// `medme_vision_free`).

import CoreGraphics
import Foundation
import ImageIO
import Vision

/// Recognize text in the image at `pathC`. Returns a malloc'd, NUL-terminated
/// UTF-8 C string of the recognized lines joined top-to-bottom by "\n"; writes
/// the average observation confidence (0.0–1.0) into `outConfidence`. The
/// caller (Rust) must free the returned pointer with `medme_vision_free`.
@_cdecl("medme_vision_recognize")
public func medme_vision_recognize(
  _ pathC: UnsafePointer<CChar>,
  _ outConfidence: UnsafeMutablePointer<Double>
) -> UnsafeMutablePointer<CChar>? {
  outConfidence.pointee = 0.0
  let path = String(cString: pathC)
  let url = URL(fileURLWithPath: path)

  // Decode the image (JPEG/PNG/TIFF/HEIC…) via ImageIO.
  guard
    let source = CGImageSourceCreateWithURL(url as CFURL, nil),
    let cgImage = CGImageSourceCreateImageAtIndex(source, 0, nil)
  else {
    return copyToCString("")
  }

  let request = VNRecognizeTextRequest()
  request.recognitionLevel = .accurate
  // Simplified + Traditional Chinese and English — covers Chinese medical
  // records (labels/units often in English) without a per-image language pick.
  request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"]
  request.usesLanguageCorrection = true

  let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
  do {
    try handler.perform([request])
  } catch {
    return copyToCString("")
  }

  let observations = request.results ?? []
  var lines: [String] = []
  var confidenceSum: Double = 0.0
  var confidenceCount = 0
  // Observations arrive in reading order (top-to-bottom); keep that order so
  // the concatenated text preserves the document's line layout.
  for observation in observations {
    guard let candidate = observation.topCandidates(1).first else { continue }
    lines.append(candidate.string)
    confidenceSum += Double(candidate.confidence)
    confidenceCount += 1
  }

  outConfidence.pointee = confidenceCount > 0 ? confidenceSum / Double(confidenceCount) : 0.0
  return copyToCString(lines.joined(separator: "\n"))
}

/// Free a buffer returned by `medme_vision_recognize`.
@_cdecl("medme_vision_free")
public func medme_vision_free(_ ptr: UnsafeMutablePointer<CChar>?) {
  if let ptr = ptr { free(ptr) }
}

/// Copy a Swift string into a freshly malloc'd C string (ownership passes to
/// the caller). `strdup` allocates with `malloc`, matching the Rust-side
/// `medme_vision_free` -> `free`.
private func copyToCString(_ s: String) -> UnsafeMutablePointer<CChar>? {
  return s.withCString { strdup($0) }
}
