//! OCR backend for MedMe: recognizes text in image bytes (png/jpg/tiff) via
//! `oar-ocr` (PP-OCRv5, ONNX Runtime). Models are auto-downloaded from
//! ModelScope into `$OAR_HOME` (default `~/.oar`) on first use, SHA-256
//! verified, and cached for subsequent runs.
//!
//! Also handles scanned/image-only PDFs (no text layer) via `recognize_pdf`:
//! it pulls page image XObjects out of the PDF with `lopdf` and OCRs each one.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage, Luma};
use imageproc::filter::gaussian_blur_f32;
use imageproc::geometric_transformations::{rotate_about_center, Border, Interpolation};
use lopdf::{Document, Object};
#[cfg(feature = "engine")]
use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
#[cfg(feature = "engine")]
use oar_ocr::utils::dynamic_to_rgb;
#[cfg(feature = "engine")]
use std::sync::OnceLock;

#[cfg(feature = "engine")]
static PIPELINE: OnceLock<OAROCR> = OnceLock::new();

/// Result of an OCR recognition call: the recognized text plus a confidence
/// score (mean of the recognized text lines' per-line confidences, `0..1`;
/// `0.0` when no lines were recognized).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrOutcome {
    pub text: String,
    pub confidence: f32,
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
    let built = OAROCRBuilder::new(
        "pp-ocrv5_mobile_det.onnx",
        "pp-ocrv5_mobile_rec.onnx",
        "ppocrv5_dict.txt",
    )
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
    reader.decode().context("ocr: decode image within limits")
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

/// Recognize text in image bytes (png/jpg/tiff/...). Returns recognized text
/// lines joined with "\n", plus a confidence score (mean of the recognized
/// lines' per-line confidences; `0.0` if no lines were recognized). Lazily
/// builds the OCR pipeline on first call (models auto-download from
/// ModelScope on first ever run on this machine).
#[cfg(feature = "engine")]
pub fn recognize(image_bytes: &[u8]) -> Result<OcrOutcome> {
    let ocr = pipeline()?;
    let dynamic = decode_image_bounded(image_bytes).context("ocr::recognize: decode image")?;
    let dynamic = preprocess(dynamic);
    let image = dynamic_to_rgb(dynamic);
    let results = ocr
        .predict(vec![image])
        .map_err(|e| anyhow::anyhow!("OCR prediction failed: {e}"))?;
    let mut lines = Vec::new();
    let mut confidences = Vec::new();
    if let Some(result) = results.into_iter().next() {
        for region in result.text_regions {
            if let Some(text) = region.text {
                if !text.trim().is_empty() {
                    lines.push(text);
                    if let Some(c) = region.confidence {
                        confidences.push(c);
                    }
                }
            }
        }
    }
    Ok(OcrOutcome {
        text: lines.join("\n"),
        confidence: mean_confidence(&confidences),
    })
}

/// No-`engine` stub (Android build): oar-ocr/ONNX Runtime isn't linked in on
/// this platform (it wouldn't work there anyway -- see the `engine` feature
/// doc in Cargo.toml). Always errors; callers already treat OCR failure as
/// non-fatal and fall back to storing the file without extracted text.
#[cfg(not(feature = "engine"))]
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
pub fn recognize_pdf(pdf_bytes: &[u8]) -> Result<OcrOutcome> {
    let doc = Document::load_mem(pdf_bytes).context("recognize_pdf: parse PDF")?;
    let mut page_texts = Vec::new();
    let mut page_confidences = Vec::new();
    for (_page_num, page_id) in doc.get_pages() {
        for image_bytes in extract_dct_images(&doc, page_id) {
            match recognize(&image_bytes) {
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
    }
    if page_texts.is_empty() {
        anyhow::bail!("recognize_pdf: no OCR-able (DCTDecode) page images found in PDF");
    }
    Ok(OcrOutcome {
        text: page_texts.join("\n"),
        confidence: mean_confidence(&page_confidences),
    })
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
}
