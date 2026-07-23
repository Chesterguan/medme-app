//! OCR backend for MedMe: recognizes text in image bytes (png/jpg/tiff) via
//! `oar-ocr` (PP-OCRv5, ONNX Runtime). Models are auto-downloaded from
//! ModelScope into `$OAR_HOME` (default `~/.oar`) on first use, SHA-256
//! verified, and cached for subsequent runs.
//!
//! Also handles scanned/image-only PDFs (no text layer) via `recognize_pdf`:
//! it pulls page image XObjects out of the PDF with `lopdf` and OCRs each one.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage, ImageDecoder, Luma};
use imageproc::filter::gaussian_blur_f32;
use imageproc::geometric_transformations::{rotate_about_center, Border, Interpolation};
use lopdf::{Document, Object};
/// macOS on-device OCR via Apple Vision — primary recognizer on the desktop
/// build (oar-ocr is the fallback). See the module for the rationale (#41).
#[cfg(target_os = "macos")]
mod vision_macos;
/// Windows on-device OCR via Windows.Media.Ocr — primary on the Windows build
/// (oar-ocr is the fallback). See the module (#41).
#[cfg(target_os = "windows")]
mod windows_ocr;
#[cfg(feature = "engine")]
use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
#[cfg(feature = "engine")]
use oar_ocr::utils::dynamic_to_rgb;
#[cfg(feature = "engine")]
use std::path::PathBuf;
#[cfg(feature = "engine")]
use std::sync::OnceLock;

#[cfg(feature = "engine")]
static PIPELINE: OnceLock<OAROCR> = OnceLock::new();

/// Optional override for where the three PP-OCRv5 model files live. When unset
/// -- which is every build we currently ship -- the builder is handed the bare
/// file names, which the `auto-download` feature resolves out of `$OAR_HOME`
/// (`~/.oar`), fetching them from ModelScope on first use. When set via
/// [`set_model_dir`], the builder gets absolute, on-disk paths instead, for
/// packaging the models alongside the binary with `auto-download` off.
#[cfg(feature = "engine")]
static MODEL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Point the OCR engine at a directory holding the three PP-OCRv5 model files
/// (`pp-ocrv5_mobile_det.onnx`, `pp-ocrv5_mobile_rec.onnx`, `ppocrv5_dict.txt`).
///
/// For packaging the models next to the binary instead of auto-downloading
/// them. In production, has no callers -- mobile does not use this crate (ADR
/// 0005), and desktop/CLI auto-download. **Test-branch exception:**
/// `feat/ios-pp-ocr-test`'s `apps/mobile_flutter/rust/src/api/vault.rs`
/// (`ensure_pp_models_ready`) calls this to point at models it writes out of
/// its own `include_bytes!`-embedded copies -- see that function for why (no
/// writable `$OAR_HOME` in the iOS sandbox). Must be called before the first
/// `recognize`/`recognize_pdf` call (the pipeline is built lazily on first use
/// and cached). Idempotent: the first call wins; later calls are ignored.
#[cfg(feature = "engine")]
pub fn set_model_dir(dir: PathBuf) {
    let _ = MODEL_DIR.set(dir);
}

/// Result of an OCR recognition call: the recognized text plus a confidence
/// score (mean of the recognized text lines' per-line confidences, `0..1`;
/// `0.0` when no lines were recognized).
/// Which OCR engine actually produced an [`OcrOutcome`]. Lets callers (the
/// ingest pipeline) record accurate provenance instead of hardcoding one
/// engine — on macOS/Windows the primary recognizer is Apple Vision /
/// Windows.Media.Ocr, not the ONNX fallback the metadata used to always claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrBackend {
    /// Apple Vision (`VNRecognizeTextRequest`) — macOS on-device.
    AppleVision,
    /// Windows.Media.Ocr (WinRT) — Windows on-device.
    WindowsOcr,
    /// oar-ocr / PP-OCRv5 ONNX engine (Linux, and the macOS/Windows fallback).
    Onnx,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrOutcome {
    pub text: String,
    pub confidence: f32,
    /// The engine that produced `text` (provenance for the vault's audit trail).
    pub backend: OcrBackend,
}

/// Mean of `Some` confidences, or `0.0` if there are none. Pure float helper
/// (no `oar-ocr` types involved), used by both `recognize` (engine-gated)
/// and `recognize_pdf` (not gated -- it just calls `recognize` per page).
fn mean_confidence(confidences: &[f32]) -> f32 {
    if confidences.is_empty() {
        0.0
    } else {
        confidences.iter().sum::<f32>() / confidences.len() as f32
    }
}

#[cfg(feature = "engine")]
fn pipeline() -> Result<&'static OAROCR> {
    if let Some(p) = PIPELINE.get() {
        return Ok(p);
    }
    // With a MODEL_DIR set, hand the builder absolute paths to packaged models
    // (for builds with `auto-download` off, where bare names wouldn't resolve).
    // Without it -- every build we ship today -- the bare names go through
    // `auto-download`'s `$OAR_HOME` resolution unchanged.
    let (det, rec, dict) = match MODEL_DIR.get() {
        Some(dir) => (
            dir.join("pp-ocrv5_mobile_det.onnx"),
            dir.join("pp-ocrv5_mobile_rec.onnx"),
            dir.join("ppocrv5_dict.txt"),
        ),
        None => (
            PathBuf::from("pp-ocrv5_mobile_det.onnx"),
            PathBuf::from("pp-ocrv5_mobile_rec.onnx"),
            PathBuf::from("ppocrv5_dict.txt"),
        ),
    };
    let built = OAROCRBuilder::new(det, rec, dict)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build OAROCR pipeline: {e}"))?;
    Ok(PIPELINE.get_or_init(|| built))
}

/// Upper bound on the pixel buffer a decoded input image may allocate. A tiny
/// crafted file can declare enormous dimensions in its header (a "pixel flood"):
/// left unbounded, `image` allocates the full raw-pixel buffer, and `preprocess`
/// then allocates several more full-resolution buffers (grayscale + f32 blur
/// intermediates) on top — OOM from a few hundred bytes of input. We decode with
/// explicit [`image::Limits`] so such inputs return `Err` instead. 512 MiB is far
/// above any real phone photo / document scan yet bounds the worst case.
#[cfg_attr(not(feature = "engine"), allow(dead_code))]
const MAX_IMAGE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// Hard ceiling on either image dimension. The alloc cap above already rejects
/// most floods, but a 1-byte-per-pixel grayscale image can declare very large
/// dimensions while staying just under it; this bounds each axis explicitly.
#[cfg_attr(not(feature = "engine"), allow(dead_code))]
const MAX_IMAGE_DIM: u32 = 20_000;

/// Working-resolution ceiling for [`preprocess`]. `decode_image_bounded` accepts
/// images up to [`MAX_IMAGE_DIM`] (20000px), but `preprocess`'s illumination
/// flattening allocates several full-resolution `f32` buffers (grayscale + blur
/// intermediates), so a ~19000px image — legal under the decode cap — balloons
/// to multiple gigabytes transiently, worst on low-RAM mobile. OCR gains nothing
/// from resolution beyond a normal scan, so we downscale (preserving aspect) to
/// this bound before those amplifying passes. A typical A4 scan at 300dpi
/// (~2500px) is already well under this and is left untouched.
const OCR_MAX_WORKING_DIM: u32 = 4_000;

/// Decode image bytes (png/jpg/tiff/...) into a [`DynamicImage`] under explicit
/// allocation + dimension limits, so a small file declaring huge dimensions
/// errors cleanly rather than driving a multi-gigabyte allocation. Behaves
/// identically to `image::load_from_memory` for normally-sized inputs.
#[cfg_attr(not(feature = "engine"), allow(dead_code))]
fn decode_image_bounded(image_bytes: &[u8]) -> Result<DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_IMAGE_ALLOC_BYTES);
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);

    let mut reader = image::ImageReader::new(std::io::Cursor::new(image_bytes))
        .with_guessed_format()
        .context("ocr: guess image format")?;
    reader.limits(limits);
    // 应用 EXIF 方向:相册/相机胶卷里的照片常带旋转标记(竖拍存成横向像素 + 一个
    // 「顺时针转 90°」的 EXIF 标记),`image` 默认**不**应用它。不修正就会把照片当
    // 横躺的像素解码 → PP 识别横躺的字 → 乱码。相机文档扫描器输出的是已摆正的图,
    // 无此标记,不受影响;无 EXIF / 拿不到方向的普通扫描件按「不变换」处理,同样不变。
    let mut decoder = reader.into_decoder().context("ocr: build decoder")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img =
        DynamicImage::from_decoder(decoder).context("ocr: decode image within limits")?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Downscale `img` (preserving aspect ratio) so neither dimension exceeds
/// [`OCR_MAX_WORKING_DIM`], returning it unchanged when it already fits. This
/// caps the transient `f32` buffers `preprocess` allocates on very large
/// inputs; OCR quality on normal-resolution scans is unaffected since they are
/// already under the limit.
fn downscale_to_working_dim(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= OCR_MAX_WORKING_DIM && h <= OCR_MAX_WORKING_DIM {
        return img;
    }
    let scale = OCR_MAX_WORKING_DIM as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    // `resize` preserves aspect and fits within the box; Triangle matches the
    // filter already used for the skew-search downscale below.
    img.resize(nw, nh, image::imageops::FilterType::Triangle)
}

/// Mild image preprocessing to bring messy phone photos of paper reports
/// closer to scan quality before OCR: grayscale, de-shadow / illumination
/// flattening, contrast stretch, and (only if a clear skew is detected) a
/// small deskew rotation. Deliberately conservative -- an already-clean scan
/// should come out looking essentially the same, just grayscale.
///
/// Never panics and never fails: any internal issue (degenerate input,
/// unreliable skew estimate, etc.) causes that step to be skipped, and if
/// something still goes wrong the original image is returned unchanged.
pub fn preprocess(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    // Too small for the blur radii / rotation search below to mean anything;
    // just pass it through rather than risk degrading a tiny image.
    if w < 16 || h < 16 {
        return img;
    }

    // Bound the working resolution before the amplifying f32 passes below
    // (`flatten_illumination` / `gaussian_blur_f32`). Normal-resolution scans
    // are under the limit and pass through untouched. See [`OCR_MAX_WORKING_DIM`].
    let img = downscale_to_working_dim(img);

    let gray = img.to_luma8();
    let flattened = flatten_illumination(&gray);
    let stretched = stretch_contrast(&flattened);
    let result = match estimate_skew_deg(&stretched) {
        Some(angle) if angle.abs() >= 0.5 && angle.is_finite() => deskew(&stretched, angle),
        _ => stretched,
    };
    DynamicImage::ImageLuma8(result)
}

/// De-shadows / normalizes uneven lighting by dividing the image by a
/// heavily-blurred copy of itself (an estimate of the local background),
/// then rescaling. Because the blur radius is much larger than a character
/// stroke, this flattens slow-varying shadows/gradients while leaving
/// fine (text-scale) detail intact.
fn flatten_illumination(gray: &GrayImage) -> GrayImage {
    let (w, h) = gray.dimensions();
    // Scale the blur radius to image size (clamped to a sane range) so this
    // behaves similarly on both small crops and large phone photos.
    let sigma = (w.min(h) as f32 / 8.0).clamp(8.0, 60.0);
    let background = gaussian_blur_f32(gray, sigma);

    let mut out = GrayImage::new(w, h);
    for ((src, bg), dst) in gray.pixels().zip(background.pixels()).zip(out.pixels_mut()) {
        let fg = src.0[0] as f32;
        let bg_v = (bg.0[0] as f32).max(1.0); // guard against div-by-zero
                                              // Rescale so a pixel matching the local background lands around
                                              // 200 (near-white, but with headroom so it isn't blown out before
                                              // the contrast-stretch step restores full range).
        let normalized = (fg / bg_v) * 200.0;
        dst.0[0] = normalized.clamp(0.0, 255.0) as u8;
    }
    out
}

/// Linearly stretches the image's intensity range to fill 0..=255. A no-op
/// on already-flat/blank images (nothing to stretch) or already-full-range
/// images.
fn stretch_contrast(gray: &GrayImage) -> GrayImage {
    let (mut lo, mut hi) = (u8::MAX, u8::MIN);
    for p in gray.pixels() {
        let v = p.0[0];
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi <= lo {
        return gray.clone();
    }
    let (w, h) = gray.dimensions();
    let lo_f = lo as f32;
    let scale = 255.0 / (hi as f32 - lo_f);
    let mut out = GrayImage::new(w, h);
    for (src, dst) in gray.pixels().zip(out.pixels_mut()) {
        let v = ((src.0[0] as f32 - lo_f) * scale).clamp(0.0, 255.0);
        dst.0[0] = v as u8;
    }
    out
}

/// Estimates the dominant text skew angle in degrees via a projection
/// profile search: for each candidate angle in a small range, rotate and
/// score by the variance of per-row pixel-intensity sums (horizontal text
/// lines produce high-contrast rows/gaps, which maximizes this variance
/// when the rotation is correct). Returns `None` when the image is too
/// small to search reliably.
///
/// The search runs on a downscaled copy since the skew angle is a global
/// property of the page and doesn't need full resolution -- this keeps the
/// O(angles x pixels) search fast even on multi-megapixel phone photos.
fn estimate_skew_deg(gray: &GrayImage) -> Option<f32> {
    let (w, h) = gray.dimensions();
    if w < 16 || h < 16 {
        return None;
    }

    let longest = w.max(h) as f32;
    let scale = 300.0 / longest;
    let small = if scale < 1.0 {
        let sw = ((w as f32 * scale).round() as u32).max(1);
        let sh = ((h as f32 * scale).round() as u32).max(1);
        image::imageops::resize(gray, sw, sh, image::imageops::FilterType::Triangle)
    } else {
        gray.clone()
    };

    const ANGLE_RANGE_DEG: f32 = 10.0;
    const ANGLE_STEP_DEG: f32 = 0.5;

    let mut best_angle = 0.0f32;
    let mut best_score = f32::MIN;
    let mut found = false;

    let mut angle_deg = -ANGLE_RANGE_DEG;
    while angle_deg <= ANGLE_RANGE_DEG {
        let rotated = rotate_about_center(
            &small,
            angle_deg.to_radians(),
            Interpolation::Nearest,
            Border::Constant(Luma([255])),
        );
        let score = row_sum_variance(&rotated);
        if score.is_finite() && score > best_score {
            best_score = score;
            best_angle = angle_deg;
            found = true;
        }
        angle_deg += ANGLE_STEP_DEG;
    }

    if !found || !best_score.is_finite() {
        return None;
    }
    Some(best_angle)
}

/// Variance of per-row summed pixel intensities -- high when rows alternate
/// between "mostly text" and "mostly gap", which is the signature of
/// correctly-oriented horizontal text.
fn row_sum_variance(img: &GrayImage) -> f32 {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return 0.0;
    }
    let sums: Vec<f64> = (0..h)
        .map(|y| (0..w).map(|x| img.get_pixel(x, y).0[0] as f64).sum::<f64>())
        .collect();
    let mean = sums.iter().sum::<f64>() / sums.len() as f64;
    let variance = sums.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / sums.len() as f64;
    variance as f32
}

/// Rotates the image clockwise by `angle_deg` about its center, filling
/// exposed corners with white (matching a paper background) rather than
/// black.
fn deskew(gray: &GrayImage, angle_deg: f32) -> GrayImage {
    rotate_about_center(
        gray,
        angle_deg.to_radians(),
        Interpolation::Bilinear,
        Border::Constant(Luma([255])),
    )
}

/// Runs the OAROCR pipeline on decoded image bytes and returns the raw
/// per-line [`oar_ocr::oarocr::TextRegion`]s (text + confidence + detection
/// box), before any joining/formatting. Shared by [`recognize_engine`] (joins
/// with "\n", the behavior every existing caller — Linux desktop/CLI primary,
/// macOS/Windows engine fallback — already depends on, unchanged here) and
/// [`recognize_engine_layout`] (mobile iOS PP-OCR test path, added for
/// feat/ios-pp-ocr-test: uses the boxes to reconstruct table columns).
#[cfg(feature = "engine")]
fn predict_regions(image_bytes: &[u8]) -> Result<Vec<oar_ocr::oarocr::TextRegion>> {
    let ocr = pipeline()?;
    let dynamic = decode_image_bounded(image_bytes).context("ocr::recognize: decode image")?;
    let dynamic = preprocess(dynamic);
    let image = dynamic_to_rgb(dynamic);
    let results = ocr
        .predict(vec![image])
        .map_err(|e| anyhow::anyhow!("OCR prediction failed: {e}"))?;
    Ok(results
        .into_iter()
        .next()
        .map(|r| r.text_regions)
        .unwrap_or_default())
}

/// Recognize text in image bytes (png/jpg/tiff/...). Returns recognized text
/// lines joined with "\n", plus a confidence score (mean of the recognized
/// lines' per-line confidences; `0.0` if no lines were recognized). Lazily
/// builds the OCR pipeline on first call (models auto-download from
/// ModelScope on first ever run on this machine).
#[cfg(feature = "engine")]
fn recognize_engine(image_bytes: &[u8]) -> Result<OcrOutcome> {
    let mut lines = Vec::new();
    let mut confidences = Vec::new();
    for region in predict_regions(image_bytes)? {
        if let Some(text) = region.text {
            if !text.trim().is_empty() {
                lines.push(text);
                if let Some(c) = region.confidence {
                    confidences.push(c);
                }
            }
        }
    }
    Ok(OcrOutcome {
        text: lines.join("\n"),
        confidence: mean_confidence(&confidences),
        backend: OcrBackend::Onnx,
    })
}

/// Same recognition as [`recognize_engine`], but the returned text has table
/// columns reconstructed from each line's detection box instead of being a
/// flat "\n"-joined dump. **Mobile iOS PP-OCRv5 test path only**
/// (feat/ios-pp-ocr-test) — every other caller keeps using [`recognize_engine`]
/// unchanged, so this cannot regress Linux/macOS/Windows output.
///
/// Mirrors the algorithm the Android path already runs in Dart
/// (`apps/mobile_flutter/lib/ocr_bridge.dart::_rebuildLayoutText`, added to fix
/// lab-report tables collapsing into a flat dump when ML Kit splits one visual
/// row into several `TextLine`s): group detection boxes into visual rows by y,
/// then within a row map each box's x position to a character column and pad
/// with spaces. PP-OCRv5's detector already produces one box per text *line*
/// (not per character or per block), the same granularity ML Kit's
/// `TextLine.boundingBox` is at, so [`rebuild_layout_text`] ports directly.
#[cfg(feature = "engine")]
pub fn recognize_engine_layout(image_bytes: &[u8]) -> Result<OcrOutcome> {
    let regions = predict_regions(image_bytes)?;
    let mut confidences = Vec::new();
    let mut layout_lines = Vec::new();
    for region in regions {
        let Some(text) = region.text else { continue };
        if text.trim().is_empty() {
            continue;
        }
        if let Some(c) = region.confidence {
            confidences.push(c);
        }
        let bb = &region.bounding_box;
        layout_lines.push(LayoutLine {
            text: text.to_string(),
            left: bb.x_min(),
            top: bb.y_min(),
            right: bb.x_max(),
            height: bb.y_max() - bb.y_min(),
        });
    }
    Ok(OcrOutcome {
        text: rebuild_layout_text(&layout_lines),
        confidence: mean_confidence(&confidences),
        backend: OcrBackend::Onnx,
    })
}

/// A recognized text line's content plus its on-page geometry (pixel
/// coordinates, origin top-left) — engine-agnostic, so [`rebuild_layout_text`]
/// doesn't depend on any one OCR crate's box type. `top`/`left`/`right` are the
/// line's bounding box edges; `height` is `bottom - top` (kept as a field
/// rather than derived, matching the ML Kit `Rect` this ports from).
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutLine {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub height: f32,
}

/// Layout-reconstruction constants — kept numerically identical to
/// `ocr_bridge.dart`'s `_ocrTargetColumnWidth` / `_ocrRowYToleranceRatio` /
/// `_ocrBlockGapRatio` so the two engines' table output lines up the same way.
const LAYOUT_TARGET_COLUMN_WIDTH: usize = 90;
const LAYOUT_ROW_Y_TOLERANCE_RATIO: f32 = 0.6;
const LAYOUT_BLOCK_GAP_RATIO: f32 = 1.6;

/// Reconstructs page layout from per-line detection boxes: lines are grouped
/// into visual rows by y-coordinate (within a tolerance relative to line
/// height); a row with a single line is emitted as-is (prose); a row with
/// multiple lines (a table row split into per-cell detections) has each
/// line's x position mapped to a character column and space-padded to align.
/// Rows separated by a much larger vertical gap than the surrounding line
/// height get a blank line between them (paragraph/table boundary).
///
/// Direct port of `ocr_bridge.dart::_rebuildLayoutText`/`_buildRowText` (see
/// that file for the original rationale) — kept as a free function here so it
/// can be unit-tested without the OCR engine and reused by any future
/// box-producing recognizer, not just PP-OCRv5.
#[cfg_attr(not(feature = "engine"), allow(dead_code))]
pub fn rebuild_layout_text(lines: &[LayoutLine]) -> String {
    let mut lines: Vec<&LayoutLine> = lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    // Reading order: top-to-bottom, then left-to-right.
    lines.sort_by(|a, b| {
        a.top
            .partial_cmp(&b.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    // 1) Group lines into visual rows by y (tolerance = a fraction of line height).
    let mut rows: Vec<Vec<&LayoutLine>> = Vec::new();
    for line in lines.iter().copied() {
        if let Some(last_row) = rows.last() {
            let ref_top = last_row[0].top;
            let tol = LAYOUT_ROW_Y_TOLERANCE_RATIO * line.height.max(last_row[0].height);
            if (line.top - ref_top).abs() <= tol {
                rows.last_mut().unwrap().push(line);
                continue;
            }
        }
        rows.push(vec![line]);
    }

    // 2) Content bounding box (not full image width) as the column-coordinate
    //    reference, same reasoning as the Dart port: avoids background margin
    //    compressing column resolution.
    let content_left = lines.iter().map(|l| l.left).fold(f32::INFINITY, f32::min);
    let content_right = lines
        .iter()
        .map(|l| l.right)
        .fold(f32::NEG_INFINITY, f32::max);
    let content_span = content_right - content_left;

    // 3) Emit each visual row; insert a blank line where the vertical gap to
    //    the previous row is much larger than the line height (block break).
    let mut out_lines: Vec<String> = Vec::new();
    let mut prev_top: Option<f32> = None;
    let mut prev_height: Option<f32> = None;
    for row in &rows {
        let row_top = row.iter().map(|l| l.top).fold(f32::INFINITY, f32::min);
        let row_height = row
            .iter()
            .map(|l| l.height)
            .fold(f32::NEG_INFINITY, f32::max);
        if let Some(pt) = prev_top {
            let ref_height = prev_height.unwrap_or(row_height);
            if ref_height > 0.0 && row_top - pt > LAYOUT_BLOCK_GAP_RATIO * ref_height {
                out_lines.push(String::new());
            }
        }
        out_lines.push(build_row_text(row, content_left, content_span));
        prev_top = Some(row_top);
        prev_height = Some(row_height);
    }
    out_lines.join("\n")
}

/// Joins one visual row's lines into a single line of text. A single-line row
/// (prose) is returned as-is; a multi-line row (one table row split into
/// several detections) has each line after the first padded with spaces so
/// its start lands at the target character column derived from its `left`
/// position within `content_span` — at least 2 spaces, matching the viewer's
/// `splitCells` "2+ consecutive spaces = column break" rule.
#[cfg_attr(not(feature = "engine"), allow(dead_code))]
fn build_row_text(row: &[&LayoutLine], content_left: f32, content_span: f32) -> String {
    if row.len() <= 1 || content_span <= 0.0001 {
        return row
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut sorted = row.to_vec();
    sorted.sort_by(|a, b| {
        a.left
            .partial_cmp(&b.left)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut buf = String::new();
    let mut buf_len = 0usize; // char count, not byte length (CJK-safe)
    for line in sorted {
        if buf.is_empty() {
            buf.push_str(&line.text);
            buf_len = line.text.chars().count();
            continue;
        }
        let col = (((line.left - content_left) / content_span) * LAYOUT_TARGET_COLUMN_WIDTH as f32)
            .round() as i64;
        let target_len = col.max(buf_len as i64 + 2).max(0) as usize;
        let pad = target_len.saturating_sub(buf_len);
        buf.push_str(&" ".repeat(pad));
        buf.push_str(&line.text);
        buf_len += pad + line.text.chars().count();
    }
    buf
}

/// macOS: hand the raw bytes to Apple Vision, which decodes them via ImageIO
/// (handles HEIC/HEIF iPhone photos + every Apple format, unlike the Rust
/// `image` crate) and recognizes the text.
#[cfg(target_os = "macos")]
fn recognize_vision(image_bytes: &[u8]) -> Result<OcrOutcome> {
    vision_macos::recognize_bytes(image_bytes)
}

/// Recognize text in image bytes. **macOS**: Apple Vision is the primary
/// recognizer (offline, strong Chinese, #41); if it errors or finds no text,
/// fall back to the oar-ocr / PP-OCRv5 engine. **Other platforms**: the engine
/// (or a stub error when the engine isn't linked in, e.g. a no-`engine` build).
#[cfg(target_os = "macos")]
pub fn recognize(image_bytes: &[u8]) -> Result<OcrOutcome> {
    match recognize_vision(image_bytes) {
        Ok(outcome) if !outcome.text.trim().is_empty() => return Ok(outcome),
        Ok(_) => {} // Vision ran but found nothing — try the engine.
        Err(e) => eprintln!("[ocr] Apple Vision failed, falling back to engine: {e:#}"),
    }
    #[cfg(feature = "engine")]
    {
        recognize_engine(image_bytes)
    }
    #[cfg(not(feature = "engine"))]
    {
        Ok(OcrOutcome {
            text: String::new(),
            confidence: 0.0,
            backend: OcrBackend::AppleVision,
        })
    }
}

/// Windows: Windows.Media.Ocr is the primary recognizer (offline, on-device,
/// #41); if it errors or finds no text, fall back to the oar-ocr / PP-OCRv5
/// engine.
#[cfg(target_os = "windows")]
pub fn recognize(image_bytes: &[u8]) -> Result<OcrOutcome> {
    match windows_ocr::recognize_bytes(image_bytes) {
        Ok(outcome) if !outcome.text.trim().is_empty() => return Ok(outcome),
        Ok(_) => {} // ran but found nothing — try the engine.
        Err(e) => eprintln!("[ocr] Windows.Media.Ocr failed, falling back to engine: {e:#}"),
    }
    #[cfg(feature = "engine")]
    {
        recognize_engine(image_bytes)
    }
    #[cfg(not(feature = "engine"))]
    {
        Ok(OcrOutcome {
            text: String::new(),
            confidence: 0.0,
            backend: OcrBackend::WindowsOcr,
        })
    }
}

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    feature = "engine"
))]
pub fn recognize(image_bytes: &[u8]) -> Result<OcrOutcome> {
    recognize_engine(image_bytes)
}

/// No-`engine`, non-native stub: nothing to recognize with. Callers treat OCR
/// failure as non-fatal (store the file without extracted text).
#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(feature = "engine")
))]
pub fn recognize(_image_bytes: &[u8]) -> Result<OcrOutcome> {
    anyhow::bail!("ocr::recognize: OCR engine not available on this platform")
}

/// OCR a PDF that has no text layer: extract each page's embedded image
/// (JPEG / `DCTDecode` XObjects -- the common encoding for App-exported
/// "image PDF" scans, e.g. Photos.app "Save as PDF" or Pillow-based
/// exporters) and OCR it via [`recognize`], joining page texts with "\n".
///
/// Only `DCTDecode`-encoded image XObjects are decoded: the stream bytes for
/// that filter are the raw JPEG, so no image-specific reconstruction is
/// needed. Other embedded-image encodings (`CCITTFaxDecode` fax scans,
/// `JPXDecode` JPEG2000, raw/Flate-encoded raster samples that would need
/// colorspace + bit-depth reconstruction) are not supported and are skipped
/// page-by-page rather than failing the whole document.
///
/// Returns an error if the PDF can't be parsed, or if no page yields any
/// non-empty OCR text. Confidence is the mean of all pages' line confidences.
/// Upper bound on how many embedded page images a single PDF will be OCR'd.
/// Each image runs a full decode + [`preprocess`] + ONNX inference — seconds of
/// CPU and hundreds of MB transiently — so a small crafted PDF declaring
/// thousands of pages/images could pin CPU and memory for minutes (a DoS). We
/// OCR at most this many and stop; anything beyond is reported as skipped rather
/// than silently dropped. 50 comfortably covers real multi-page reports.
const MAX_OCR_PAGE_IMAGES: usize = 50;

/// OCR each image in `images` via `recognize_one`, but run the (expensive)
/// recognizer on at most [`MAX_OCR_PAGE_IMAGES`] of them. Returns the collected
/// per-page texts, their confidences, and the count of images that were NOT
/// OCR'd because the cap was reached.
///
/// The iterator is consumed lazily one image at a time (each is dropped
/// immediately once past the cap), so both the OCR work and the peak memory
/// stay bounded regardless of how many images the document declares. Kept
/// separate from [`recognize_pdf`] so the cap is unit-testable without a real
/// multi-image PDF or the OCR engine.
fn ocr_page_images<I, F>(images: I, mut recognize_one: F) -> (Vec<String>, Vec<f32>, usize)
where
    I: IntoIterator<Item = Vec<u8>>,
    F: FnMut(&[u8]) -> Result<OcrOutcome>,
{
    let mut page_texts = Vec::new();
    let mut page_confidences = Vec::new();
    let mut processed = 0usize;
    let mut skipped = 0usize;
    for image_bytes in images {
        if processed >= MAX_OCR_PAGE_IMAGES {
            // Past the cap: don't run OCR, just tally so we can report honestly.
            skipped += 1;
            continue;
        }
        processed += 1;
        match recognize_one(&image_bytes) {
            Ok(outcome) if !outcome.text.trim().is_empty() => {
                page_confidences.push(outcome.confidence);
                page_texts.push(outcome.text);
            }
            Ok(_) => {}
            Err(e) => {
                // One image failing OCR shouldn't sink the other pages.
                eprintln!("recognize_pdf: OCR failed for one page image: {e:#}");
            }
        }
    }
    (page_texts, page_confidences, skipped)
}

pub fn recognize_pdf(pdf_bytes: &[u8]) -> Result<OcrOutcome> {
    let doc = Document::load_mem(pdf_bytes).context("recognize_pdf: parse PDF")?;
    // Lazily stream every page's DCTDecode images; the cap is enforced (and
    // peak memory bounded) inside `ocr_page_images`.
    let images = doc
        .get_pages()
        .into_values()
        .flat_map(|page_id| extract_dct_images(&doc, page_id));
    let (page_texts, page_confidences, skipped) = ocr_page_images(images, recognize);
    if skipped > 0 {
        // No silent truncation: make it visible that we stopped early on purpose.
        eprintln!(
            "recognize_pdf: OCR capped at {MAX_OCR_PAGE_IMAGES} page images to bound work; \
             {skipped} additional embedded image(s) were NOT OCR'd"
        );
    }
    if page_texts.is_empty() {
        anyhow::bail!("recognize_pdf: no OCR-able (DCTDecode) page images found in PDF");
    }
    Ok(OcrOutcome {
        text: page_texts.join("\n"),
        confidence: mean_confidence(&page_confidences),
        // Page images went through `recognize`, whose primary engine is
        // platform-fixed; label with that platform's engine.
        backend: pdf_ocr_backend(),
    })
}

/// The primary OCR engine `recognize` uses on this platform — used to label
/// [`recognize_pdf`]'s aggregate outcome (its page images all go through
/// `recognize`).
#[inline]
fn pdf_ocr_backend() -> OcrBackend {
    #[cfg(target_os = "macos")]
    {
        OcrBackend::AppleVision
    }
    #[cfg(target_os = "windows")]
    {
        OcrBackend::WindowsOcr
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        OcrBackend::Onnx
    }
}

/// Collect raw JPEG bytes for every `DCTDecode` image XObject directly
/// referenced by a page's `/Resources /XObject` dict. Does not recurse into
/// Form XObjects.
fn extract_dct_images(doc: &Document, page_id: lopdf::ObjectId) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let resources = match doc.get_page_resources(page_id) {
        Ok((Some(dict), _)) => dict,
        _ => return out,
    };
    let xobjects = match resources.get(b"XObject").and_then(Object::as_dict) {
        Ok(d) => d.clone(),
        Err(_) => return out,
    };
    for (_name, obj_ref) in xobjects.iter() {
        let Object::Reference(oid) = obj_ref else {
            continue;
        };
        let Ok(Object::Stream(stream)) = doc.get_object(*oid) else {
            continue;
        };
        let is_image =
            stream.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice());
        if !is_image {
            continue;
        }
        let filters = stream.filters().unwrap_or_default();
        if filters.len() == 1 && filters[0] == b"DCTDecode" {
            out.push(stream.content.clone());
        }
        // Other filters not handled -- see doc comment on recognize_pdf.
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal IEEE CRC-32 (used to forge a valid PNG IHDR chunk below).
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Builds a byte-tiny but structurally-valid PNG whose IHDR declares
    /// `w`x`h` RGB pixels. The image decoder reads these dimensions from the
    /// header and would allocate `w*h*3` bytes for the raw buffer — this is the
    /// classic "pixel flood": a few dozen bytes of input demanding gigabytes.
    fn png_with_declared_dimensions(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, color type 2 (RGB)
        out.extend_from_slice(&13u32.to_be_bytes()); // IHDR data length
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        // A single empty IDAT + IEND so the stream is well-formed up to the point
        // the size check fires (it fires before any IDAT is read).
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"IDAT");
        out.extend_from_slice(&crc32(b"IDAT").to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&crc32(b"IEND").to_be_bytes());
        out
    }

    #[test]
    fn decode_rejects_pixel_flood_instead_of_ooming() {
        // ~46000 x 46000 x 3 = ~6.3 GB demanded from ~50 bytes of input. With the
        // bounded decoder this returns Err; unbounded, it would try to allocate
        // gigabytes (OOM) before `preprocess` ever ran.
        let bomb = png_with_declared_dimensions(46_000, 46_000);
        assert!(bomb.len() < 128, "the bomb file itself is tiny");
        let err = decode_image_bounded(&bomb).expect_err("pixel flood must be rejected");
        // "Memory limit exceeded" / "Image size exceeds limit" — proves the IHDR
        // parsed and the *size* guard fired (not a CRC/format rejection).
        let msg = format!("{err:#}");
        assert!(
            msg.contains("limit"),
            "expected a decode-limit error, got: {msg}"
        );
    }

    #[test]
    fn decode_accepts_normal_image() {
        // A small, real image decodes identically to before (behavior preserved).
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(64, 48, Luma([200])));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png)
            .expect("encode png");
        let decoded = decode_image_bounded(png.get_ref()).expect("normal image decodes");
        assert_eq!(decoded.dimensions(), (64, 48));
    }

    #[test]
    fn mean_confidence_of_empty_is_zero() {
        assert_eq!(mean_confidence(&[]), 0.0);
    }

    #[test]
    fn mean_confidence_averages_values() {
        assert_eq!(
            mean_confidence(&[0.8, 0.6, 1.0]),
            (0.8 + 0.6 + 1.0f32) / 3.0
        );
    }

    /// Builds a synthetic "document" image: a white background with evenly
    /// spaced horizontal black bars (standing in for text lines), plus a
    /// left-to-right lighting gradient (standing in for an uneven-shadow
    /// phone photo).
    fn synthetic_document(w: u32, h: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let is_bar = (y % 12) < 3;
            let base: f32 = if is_bar { 40.0 } else { 235.0 };
            // Shadow gradient: darker on the left, brighter on the right.
            let shadow = 0.55 + 0.45 * (x as f32 / w.max(1) as f32);
            Luma([(base * shadow).clamp(0.0, 255.0) as u8])
        })
    }

    #[test]
    fn preprocess_handles_tiny_image_without_panicking() {
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(4, 4, Luma([128])));
        let out = preprocess(img.clone());
        // Below the size floor: passed through unchanged.
        assert_eq!(out.dimensions(), img.dimensions());
    }

    #[test]
    fn preprocess_of_synthetic_photo_is_same_size_and_finite() {
        let doc = synthetic_document(160, 120);
        let img = DynamicImage::ImageLuma8(doc);
        let out = preprocess(img.clone());
        assert_eq!(out.dimensions(), img.dimensions());
        // Every output pixel is a valid, non-degenerate u8 (implicitly true
        // for GrayImage) -- just make sure we actually got real content out,
        // not an all-zero or all-saturated image.
        let gray = out.to_luma8();
        let (mut lo, mut hi) = (255u8, 0u8);
        for p in gray.pixels() {
            lo = lo.min(p.0[0]);
            hi = hi.max(p.0[0]);
        }
        assert!(hi > lo, "expected some contrast in preprocessed output");
    }

    #[test]
    fn preprocess_on_uniform_image_does_not_panic() {
        // A blank/solid-color "page": no text, no gradient. Should pass
        // through the pipeline safely (contrast stretch is a no-op here).
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(200, 150, Luma([255])));
        let out = preprocess(img.clone());
        assert_eq!(out.dimensions(), img.dimensions());
    }

    #[test]
    fn flatten_illumination_reduces_shadow_gradient() {
        let doc = synthetic_document(160, 120);
        let flattened = flatten_illumination(&doc);
        // Compare mean brightness of the "background" (non-bar) rows on the
        // dark (left) side vs the bright (right) side before and after.
        let side_means = |img: &GrayImage| -> (f64, f64) {
            let (w, h) = img.dimensions();
            let mut left = (0u64, 0u64);
            let mut right = (0u64, 0u64);
            for y in 0..h {
                if (y % 12) < 3 {
                    continue; // skip bar rows, only look at background
                }
                for x in 0..w {
                    let v = img.get_pixel(x, y).0[0] as u64;
                    if x < w / 2 {
                        left.0 += v;
                        left.1 += 1;
                    } else {
                        right.0 += v;
                        right.1 += 1;
                    }
                }
            }
            (
                left.0 as f64 / left.1.max(1) as f64,
                right.0 as f64 / right.1.max(1) as f64,
            )
        };
        let (before_left, before_right) = side_means(&doc);
        let (after_left, after_right) = side_means(&flattened);
        let before_gap = (before_right - before_left).abs();
        let after_gap = (after_right - after_left).abs();
        assert!(
            after_gap < before_gap,
            "expected shadow gradient to shrink: before={before_gap}, after={after_gap}"
        );
    }

    #[test]
    fn stretch_contrast_expands_narrow_range_to_full() {
        // A low-contrast image using only the middle of the range.
        let img = GrayImage::from_fn(50, 50, |x, _y| Luma([if x < 25 { 100u8 } else { 140u8 }]));
        let out = stretch_contrast(&img);
        let (mut lo, mut hi) = (255u8, 0u8);
        for p in out.pixels() {
            lo = lo.min(p.0[0]);
            hi = hi.max(p.0[0]);
        }
        assert_eq!(lo, 0);
        assert_eq!(hi, 255);
    }

    #[test]
    fn stretch_contrast_is_noop_on_flat_image() {
        let img = GrayImage::from_pixel(30, 30, Luma([77]));
        let out = stretch_contrast(&img);
        assert!(out.pixels().all(|p| p.0[0] == 77));
    }

    #[test]
    fn estimate_skew_deg_none_on_tiny_image() {
        let img = GrayImage::from_pixel(8, 8, Luma([255]));
        assert_eq!(estimate_skew_deg(&img), None);
    }

    #[test]
    fn deskew_of_rotated_synthetic_recovers_near_zero_residual_skew() {
        let doc = synthetic_document(240, 180);
        let skew_deg = 5.0f32;
        let skewed = rotate_about_center(
            &doc,
            skew_deg.to_radians(),
            Interpolation::Bilinear,
            Border::Constant(Luma([255])),
        );

        let estimated = estimate_skew_deg(&skewed).expect("should find a skew estimate");
        assert!(estimated.is_finite());

        let corrected = deskew(&skewed, estimated);
        assert_eq!(corrected.dimensions(), skewed.dimensions());

        // Residual skew after correction should be small: re-estimating on
        // the corrected image should find an angle close to 0.
        let residual = estimate_skew_deg(&corrected).unwrap_or(0.0);
        assert!(
            residual.abs() <= 1.5,
            "expected small residual skew after deskew, got {residual} (estimated correction was {estimated})"
        );
    }

    #[test]
    fn estimate_skew_deg_on_unskewed_image_is_near_zero() {
        let doc = synthetic_document(240, 180);
        let angle = estimate_skew_deg(&doc).unwrap_or(0.0);
        assert!(
            angle.abs() <= 1.0,
            "expected near-zero skew estimate for unrotated image, got {angle}"
        );
    }

    #[test]
    fn ocr_page_images_caps_expensive_work_and_reports_skips() {
        // More images than the cap: the recognizer (the expensive part) must run
        // at most MAX_OCR_PAGE_IMAGES times, and the remainder must be reported
        // as skipped -- not silently dropped.
        let extra = 7;
        let total = MAX_OCR_PAGE_IMAGES + extra;
        let images: Vec<Vec<u8>> = (0..total).map(|i| vec![i as u8]).collect();
        let mut calls = 0usize;
        let (texts, confs, skipped) = ocr_page_images(images, |_bytes| {
            calls += 1;
            Ok(OcrOutcome {
                text: "line".to_string(),
                confidence: 1.0,
                backend: OcrBackend::Onnx,
            })
        });
        assert_eq!(
            calls, MAX_OCR_PAGE_IMAGES,
            "OCR must run on at most the cap-many images"
        );
        assert_eq!(texts.len(), MAX_OCR_PAGE_IMAGES);
        assert_eq!(confs.len(), MAX_OCR_PAGE_IMAGES);
        assert_eq!(
            skipped, extra,
            "images beyond the cap must be reported as skipped"
        );
    }

    #[test]
    fn ocr_page_images_under_cap_processes_all_with_no_skips() {
        // A normal document (few images) is unaffected: everything is OCR'd and
        // nothing is reported as skipped.
        let images: Vec<Vec<u8>> = (0..3).map(|_| vec![0u8]).collect();
        let mut calls = 0usize;
        let (texts, _confs, skipped) = ocr_page_images(images, |_bytes| {
            calls += 1;
            Ok(OcrOutcome {
                text: "ok".to_string(),
                confidence: 0.5,
                backend: OcrBackend::Onnx,
            })
        });
        assert_eq!(calls, 3);
        assert_eq!(texts.len(), 3);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn downscale_shrinks_oversized_image_preserving_aspect() {
        // An 8000x4000 image (legal under the 20000px decode cap) is brought
        // under the working limit before the amplifying f32 passes run.
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(8000, 4000, Luma([128])));
        let out = downscale_to_working_dim(img);
        let (w, h) = out.dimensions();
        assert!(
            w <= OCR_MAX_WORKING_DIM && h <= OCR_MAX_WORKING_DIM,
            "expected both dims <= {OCR_MAX_WORKING_DIM}, got {w}x{h}"
        );
        assert_eq!(w, OCR_MAX_WORKING_DIM, "longest axis should hit the limit");
        // 2:1 aspect ratio preserved.
        assert!(
            (w as f32 / h as f32 - 2.0).abs() < 0.05,
            "aspect ratio should be preserved, got {w}x{h}"
        );
    }

    #[test]
    fn downscale_leaves_normal_scan_untouched() {
        // A typical A4 scan at 300dpi (~2480x3508) is under the limit and must
        // be returned with identical dimensions (behavior preserved).
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(2480, 3508, Luma([128])));
        let out = downscale_to_working_dim(img);
        assert_eq!(out.dimensions(), (2480, 3508));
    }

    #[test]
    fn preprocess_downscales_oversized_input_below_working_dim() {
        // End-to-end through preprocess: an oversized input comes out bounded to
        // the working dimension (so the f32 buffers never see full resolution).
        let big = DynamicImage::ImageLuma8(GrayImage::from_pixel(
            OCR_MAX_WORKING_DIM + 2000,
            120,
            Luma([200]),
        ));
        let out = preprocess(big);
        let (w, h) = out.dimensions();
        assert!(
            w <= OCR_MAX_WORKING_DIM && h <= OCR_MAX_WORKING_DIM,
            "preprocess output should be bounded, got {w}x{h}"
        );
    }

    /// Requires network access to ModelScope on first run (models are cached
    /// afterward in $OAR_HOME). Run explicitly with:
    ///   cargo test -p ocr -- --ignored
    #[test]
    #[ignore]
    fn recognizes_cjk_test_image() {
        let bytes = std::fs::read("/tmp/ocr_test.png")
            .expect("generate /tmp/ocr_test.png first (see feat-ocr-report.md)");
        let outcome = recognize(&bytes).expect("OCR should succeed");
        assert!(
            outcome.text.contains("Creatinine") || outcome.text.contains("肌酐"),
            "unexpected OCR text: {}",
            outcome.text
        );
        assert!(
            outcome.confidence > 0.0,
            "expected non-zero confidence, got {}",
            outcome.confidence
        );
    }

    /// Requires network access to ModelScope on first run (models are cached
    /// afterward in $OAR_HOME). Run explicitly with:
    ///   cargo test -p ocr -- --ignored
    #[test]
    #[ignore]
    fn recognizes_scanned_image_pdf() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/photos/2026-03-15_检验报告_扫描图PDF.pdf"
        );
        let bytes = std::fs::read(path).expect("demo scanned PDF present");
        let outcome = recognize_pdf(&bytes).expect("recognize_pdf should succeed");
        assert!(
            outcome.text.contains("肌酐") || outcome.text.contains("Creatinine"),
            "unexpected OCR text: {}",
            outcome.text
        );
        assert!(
            outcome.text.contains("2026-03-15"),
            "expected date in OCR text: {}",
            outcome.text
        );
        assert!(
            outcome.confidence > 0.0,
            "expected non-zero confidence, got {}",
            outcome.confidence
        );
    }

    fn ll(text: &str, left: f32, top: f32, right: f32, height: f32) -> LayoutLine {
        LayoutLine {
            text: text.to_string(),
            left,
            top,
            right,
            height,
        }
    }

    #[test]
    fn rebuild_layout_text_of_empty_is_empty() {
        assert_eq!(rebuild_layout_text(&[]), "");
    }

    #[test]
    fn rebuild_layout_text_single_line_per_row_passes_through() {
        // Prose: one detection box per visual row -> lines emitted unchanged,
        // in top-to-bottom order, no padding introduced.
        let lines = vec![
            ll("第二行", 10.0, 40.0, 100.0, 20.0),
            ll("第一行", 10.0, 10.0, 100.0, 20.0),
        ];
        assert_eq!(rebuild_layout_text(&lines), "第一行\n第二行");
    }

    #[test]
    fn rebuild_layout_text_aligns_multi_box_row_into_columns() {
        // A lab-report row split into 3 detections at increasing x — must be
        // joined into one line with >=2 spaces between fields (splitCells rule).
        let lines = vec![
            ll("肌酐", 0.0, 100.0, 40.0, 20.0),
            ll("88", 200.0, 102.0, 230.0, 20.0),
            ll("umol/L", 400.0, 101.0, 480.0, 20.0),
        ];
        let out = rebuild_layout_text(&lines);
        assert_eq!(out.lines().count(), 1, "one visual row -> one output line");
        assert!(out.starts_with("肌酐"));
        // Every field after the first must be separated by >=2 spaces.
        for part in ["88", "umol/L"] {
            let idx = out.find(part).expect("field present");
            let gap = out[..idx].chars().rev().take_while(|c| *c == ' ').count();
            assert!(
                gap >= 2,
                "expected >=2 space gap before {part:?}, got {gap} in {out:?}"
            );
        }
    }

    #[test]
    fn rebuild_layout_text_inserts_blank_line_at_large_vertical_gap() {
        // Second row's top is far below the first row's line height -> treated
        // as a block/table boundary, blank line inserted between them.
        let lines = vec![
            ll("段落一", 0.0, 0.0, 100.0, 20.0),
            ll("段落二", 0.0, 200.0, 100.0, 20.0),
        ];
        let out = rebuild_layout_text(&lines);
        assert_eq!(out, "段落一\n\n段落二");
    }

    #[test]
    fn rebuild_layout_text_groups_close_y_into_same_row() {
        // Two boxes with nearly identical `top` (within tolerance) count as
        // the same visual row even though they were pushed in arbitrary order.
        let lines = vec![
            ll("B", 300.0, 12.0, 340.0, 20.0),
            ll("A", 0.0, 10.0, 40.0, 20.0),
        ];
        let out = rebuild_layout_text(&lines);
        assert_eq!(
            out.lines().count(),
            1,
            "close-y boxes must merge into one row"
        );
    }
}
