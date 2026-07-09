//! DICOM (.dcm) support: metadata parsing + rendered PNG preview.
//!
//! v0.1 scope (see docs/010_Imaging_DICOM.md): parse the handful of tags MedMe
//! needs to file a DICOM instance as an imaging document (no OCR needed —
//! DICOM carries structured metadata), and render a single representative
//! frame to an 8-bit windowed grayscale PNG for viewing.

use anyhow::Context;
use dicom_object::{FileDicomObject, InMemDicomObject};
use dicom_pixeldata::{ConvertOptions, PixelDecoder};
use serde::Serialize;
use std::io::Cursor;

/// Metadata extracted from a DICOM instance's tags (no pixel decoding).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DicomMeta {
    /// Modality (0008,0060) — e.g. "CT", "MR", "US", "CR", "DX".
    pub modality: Option<String>,
    /// StudyDate (0008,0020), parsed from DICOM "YYYYMMDD" into an RFC3339
    /// UTC-midnight string (e.g. "2004-01-19T00:00:00+00:00").
    pub study_date: Option<String>,
    /// StudyDescription (0008,1030).
    pub description: Option<String>,
    /// BodyPartExamined (0018,0015).
    pub body_part: Option<String>,
    /// InstitutionName (0008,0080).
    pub institution: Option<String>,
    /// PatientName (0010,0010).
    pub patient_name: Option<String>,
    /// PatientSex (0010,0040).
    pub patient_sex: Option<String>,
    /// AccessionNumber (0008,0050).
    pub accession: Option<String>,
    /// StudyInstanceUID (0020,000D).
    pub study_uid: Option<String>,
    /// SeriesInstanceUID (0020,000E).
    pub series_uid: Option<String>,
    /// SeriesNumber (0020,0011), parsed as an integer.
    pub series_number: Option<i32>,
    /// InstanceNumber (0020,0013), parsed as an integer — the slice's order
    /// within its series.
    pub instance_number: Option<i32>,
    /// SeriesDescription (0008,103E).
    pub series_description: Option<String>,
}

/// Reads a named element as a trimmed, non-empty string, if present.
fn tag_str(obj: &FileDicomObject<InMemDicomObject>, name: &str) -> Option<String> {
    obj.element_by_name(name)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Reads a named element as an integer (DICOM IS values are decimal strings,
/// sometimes space-padded), if present and parseable.
fn tag_int(obj: &FileDicomObject<InMemDicomObject>, name: &str) -> Option<i32> {
    tag_str(obj, name).and_then(|s| s.parse().ok())
}

/// Parses DICOM "YYYYMMDD" (StudyDate) into an RFC3339 UTC-midnight string.
/// Returns `None` if the input isn't exactly 8 digits or isn't a valid date.
fn parse_study_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = raw[0..4].parse().ok()?;
    let month: u32 = raw[4..6].parse().ok()?;
    let day: u32 = raw[6..8].parse().ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let dt = date.and_hms_opt(0, 0, 0)?;
    let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc);
    Some(utc.to_rfc3339())
}

/// Parses the DICOM tags MedMe needs from a raw `.dcm` byte buffer.
///
/// Reads the standard on-disk structure (128-byte preamble + `DICM` magic +
/// file meta group + data set); preamble detection is automatic so this also
/// tolerates meta-group-only streams.
pub fn parse_meta(dcm_bytes: &[u8]) -> anyhow::Result<DicomMeta> {
    let obj = dicom_object::from_reader(Cursor::new(dcm_bytes))
        .context("failed to parse DICOM object")?;

    Ok(DicomMeta {
        modality: tag_str(&obj, "Modality"),
        study_date: tag_str(&obj, "StudyDate").and_then(|s| parse_study_date(&s)),
        description: tag_str(&obj, "StudyDescription"),
        body_part: tag_str(&obj, "BodyPartExamined"),
        institution: tag_str(&obj, "InstitutionName"),
        patient_name: tag_str(&obj, "PatientName"),
        patient_sex: tag_str(&obj, "PatientSex"),
        accession: tag_str(&obj, "AccessionNumber"),
        study_uid: tag_str(&obj, "StudyInstanceUID"),
        series_uid: tag_str(&obj, "SeriesInstanceUID"),
        series_number: tag_int(&obj, "SeriesNumber"),
        instance_number: tag_int(&obj, "InstanceNumber"),
        series_description: tag_str(&obj, "SeriesDescription"),
    })
}

/// Upper bound on the number of decoded pixel bytes we will allocate for one
/// DICOM instance, computed from the header BEFORE any pixel decoding. A tiny
/// crafted file can declare enormous Rows/Columns/NumberOfFrames in its header;
/// without this guard those values size the decode buffers directly, forcing a
/// multi-gigabyte allocation (or, for compressed transfer syntaxes decoded via
/// C FFI — JPEG 2000 / JPEG-LS — a decompression bomb) before we ever touch the
/// pixels. 512 MiB is far above any real single medical image yet bounds the
/// worst case. This cap applies regardless of the `codecs` feature.
pub const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;

/// Reads the pixel-geometry header tags (Rows 0028,0010 × Columns 0028,0011 ×
/// BitsAllocated 0028,0100 × SamplesPerPixel 0028,0002 × NumberOfFrames
/// 0028,0008) and rejects the instance when the declared decoded size exceeds
/// [`MAX_DECODE_BYTES`] — BEFORE any decode/allocation. Missing tags default
/// conservatively (SamplesPerPixel=1, BitsAllocated=8, NumberOfFrames=1) so a
/// header that omits them cannot slip past the cap, and the multiplication is
/// saturating so an overflowing product is treated as "over cap".
fn check_decode_bounds(obj: &FileDicomObject<InMemDicomObject>) -> anyhow::Result<()> {
    let tag_u64 = |name: &str, default: u64| -> u64 {
        obj.element_by_name(name)
            .ok()
            .and_then(|e| e.to_int::<u64>().ok())
            .unwrap_or(default)
    };
    let rows = tag_u64("Rows", 0);
    let cols = tag_u64("Columns", 0);
    let samples = tag_u64("SamplesPerPixel", 1).max(1);
    let bits = tag_u64("BitsAllocated", 8).max(1);
    let bytes_per_sample = bits.div_ceil(8);
    let frames = tag_u64("NumberOfFrames", 1).max(1);

    let total = rows
        .checked_mul(cols)
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .and_then(|v| v.checked_mul(samples))
        .and_then(|v| v.checked_mul(frames))
        .unwrap_or(u64::MAX);

    if total > MAX_DECODE_BYTES {
        anyhow::bail!(
            "DICOM declares {total} bytes of pixel data \
             ({rows}x{cols}, {samples} sample(s), {bytes_per_sample} B/sample, {frames} frame(s)), \
             exceeding the {MAX_DECODE_BYTES}-byte decode cap"
        );
    }
    Ok(())
}

/// Decodes the first frame's pixel data and renders it as an 8-bit,
/// windowed (VOI LUT applied when present) grayscale PNG.
///
/// Errors if the object has no pixel data, or the pixel data can't be
/// decoded (e.g. an unsupported compressed transfer syntax).
pub fn render_png(dcm_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let obj = dicom_object::from_reader(Cursor::new(dcm_bytes))
        .context("failed to parse DICOM object")?;
    // Reject oversized/bomb dimensions from the header before allocating.
    check_decode_bounds(&obj)?;
    let pixel_data = obj
        .decode_pixel_data()
        .context("failed to decode DICOM pixel data")?;
    let opts = ConvertOptions::new().force_8bit();
    let image = pixel_data
        .to_dynamic_image_with_options(0, &opts)
        .context("failed to render DICOM pixel data to an image")?;

    let mut png_bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut png_bytes, image::ImageFormat::Png)
        .context("failed to encode PNG")?;
    Ok(png_bytes.into_inner())
}

/// Header describing a single decoded DICOM frame's raw pixels, so the frontend
/// can interpret the accompanying byte buffer and apply window/level itself.
///
/// Mirrors what `DicomViewer.tsx` needs for its shared window/level + canvas
/// draw path: grayscale frames carry raw (un-rescaled) samples as little-endian
/// `u8` / `u16` / `i16` (per `bits_allocated` + `pixel_representation`) plus the
/// modality rescale + any VOI window so the client applies them; color frames
/// carry interleaved 8-bit RGB (`samples_per_pixel == 3`, `photometric == "RGB"`).
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct DecodedFrameHeader {
    pub rows: u32,
    pub cols: u32,
    pub samples_per_pixel: u16,
    pub bits_allocated: u16,
    pub bits_stored: u16,
    /// 0 = unsigned samples, 1 = signed (two's-complement) samples.
    pub pixel_representation: u16,
    /// Photometric interpretation of the returned pixels: "MONOCHROME1",
    /// "MONOCHROME2", or "RGB" (color is always converted to interleaved RGB).
    pub photometric: String,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    /// Default VOI window center (0028,1050), when the object defines one.
    pub window_center: Option<f64>,
    /// Default VOI window width (0028,1051), when the object defines one.
    pub window_width: Option<f64>,
}

/// A decoded frame: its [`DecodedFrameHeader`] plus the raw pixel bytes.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub header: DecodedFrameHeader,
    /// Little-endian grayscale samples (u8/u16/i16) or interleaved RGB8, per
    /// `header`. Length = rows * cols * samples_per_pixel * ceil(bits_allocated/8).
    pub pixels: Vec<u8>,
}

impl DecodedFrame {
    /// Wire format for one IPC round-trip: 4-byte little-endian header length,
    /// then the UTF-8 JSON [`DecodedFrameHeader`], then the raw pixel bytes.
    /// The frontend slices the buffer back apart by the leading length.
    pub fn into_ipc_bytes(self) -> anyhow::Result<Vec<u8>> {
        let json = serde_json::to_vec(&self.header).context("failed to serialize frame header")?;
        let mut out = Vec::with_capacity(4 + json.len() + self.pixels.len());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&self.pixels);
        Ok(out)
    }
}

/// Decodes a single frame of a DICOM instance to raw pixels for the interactive
/// viewer — handles ANY transfer syntax the enabled `dicom-pixeldata` codecs
/// support (uncompressed, JPEG baseline, JPEG 2000, JPEG-LS, RLE Lossless).
///
/// Grayscale frames return raw (un-rescaled) little-endian samples so the client
/// applies rescale + window/level for diagnostic-correct display; color frames
/// return interleaved RGB8 (YBR is converted to RGB during decode).
pub fn decode_frame(dcm_bytes: &[u8], frame_index: u32) -> anyhow::Result<DecodedFrame> {
    use dicom_pixeldata::{PhotometricInterpretation, PixelRepresentation};

    let obj = dicom_object::from_reader(Cursor::new(dcm_bytes))
        .context("failed to parse DICOM object")?;
    // Reject oversized/bomb dimensions from the header before allocating.
    check_decode_bounds(&obj)?;
    // Decode just this frame (cheap for large multi-frame series; keeps
    // scrolling + neighbor prefetch responsive).
    let decoded = obj
        .decode_pixel_data_frame(frame_index)
        .context("failed to decode DICOM pixel data")?;

    let rows = decoded.rows();
    let cols = decoded.columns();
    let samples_per_pixel = decoded.samples_per_pixel();
    let bits_allocated = decoded.bits_allocated();
    let bits_stored = decoded.bits_stored();

    // Modality rescale (first/only entry — we decoded a single frame).
    let (rescale_slope, rescale_intercept) = decoded
        .rescale()
        .ok()
        .and_then(|r| r.first())
        .map(|r| (r.slope, r.intercept))
        .unwrap_or((1.0, 0.0));

    // Default VOI window, if the object carries one.
    let (window_center, window_width) = decoded
        .window()
        .ok()
        .flatten()
        .and_then(|w| w.first())
        .map(|w| (Some(w.center), Some(w.width)))
        .unwrap_or((None, None));

    let is_color = samples_per_pixel >= 3;
    let (photometric, pixels) = if is_color {
        // Color: let the codec normalize (YBR→RGB, planar→interleaved) and hand
        // the canvas straight 8-bit RGB.
        let img = decoded
            .to_dynamic_image(0)
            .context("failed to render color frame")?
            .to_rgb8();
        ("RGB".to_string(), img.into_raw())
    } else {
        // Grayscale: raw native-endian samples (little-endian on all supported
        // targets), un-rescaled — the client applies rescale + window/level.
        let photometric = match decoded.photometric_interpretation() {
            PhotometricInterpretation::Monochrome1 => "MONOCHROME1",
            _ => "MONOCHROME2",
        }
        .to_string();
        let bytes = decoded
            .frame_data(0)
            .context("failed to read decoded frame samples")?
            .to_vec();
        (photometric, bytes)
    };

    let pixel_representation = match decoded.pixel_representation() {
        PixelRepresentation::Signed => 1,
        PixelRepresentation::Unsigned => 0,
    };

    Ok(DecodedFrame {
        header: DecodedFrameHeader {
            rows,
            cols,
            samples_per_pixel: if is_color { 3 } else { 1 },
            bits_allocated: if is_color { 8 } else { bits_allocated },
            bits_stored: if is_color { 8 } else { bits_stored },
            pixel_representation,
            photometric,
            rescale_slope,
            rescale_intercept,
            window_center,
            window_width,
        },
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/dicom/"
        ))
        .join(name);
        std::fs::read(&path)
            .unwrap_or_else(|e| panic!("failed to read sample {}: {e}", path.display()))
    }

    #[test]
    fn parses_ct_small_metadata() {
        let bytes = sample("CT_small.dcm");
        let meta = parse_meta(&bytes).unwrap();
        assert_eq!(meta.modality.as_deref(), Some("CT"));
        assert_eq!(
            meta.study_date.as_deref(),
            Some("2004-01-19T00:00:00+00:00")
        );
        assert_eq!(meta.institution.as_deref(), Some("JFK IMAGING CENTER"));
        assert_eq!(meta.patient_name.as_deref(), Some("CompressedSamples^CT1"));
        assert_eq!(meta.patient_sex.as_deref(), Some("O"));
        assert!(meta.study_uid.is_some());
    }

    #[test]
    fn parses_series_and_instance_fields() {
        let bytes = sample("CT_small.dcm");
        let meta = parse_meta(&bytes).unwrap();
        // Series/Instance grouping fields (imaging overhaul P1): these drive
        // Study→Series→Instance grouping + slice-stack ordering.
        assert!(meta.series_uid.is_some(), "SeriesInstanceUID should parse");
        assert_ne!(
            meta.series_uid, meta.study_uid,
            "series UID differs from study UID"
        );
        assert_eq!(meta.series_number, Some(1));
        assert_eq!(meta.instance_number, Some(1));
    }

    #[test]
    fn parses_mr_small_metadata() {
        let bytes = sample("MR_small.dcm");
        let meta = parse_meta(&bytes).unwrap();
        assert_eq!(meta.modality.as_deref(), Some("MR"));
        assert_eq!(
            meta.study_date.as_deref(),
            Some("2004-08-26T00:00:00+00:00")
        );
        assert_eq!(meta.institution.as_deref(), Some("TOSHIBA"));
        assert_eq!(meta.patient_sex.as_deref(), Some("F"));
    }

    #[test]
    fn renders_ct_small_to_valid_png() {
        let bytes = sample("CT_small.dcm");
        let png = render_png(&bytes).unwrap();
        assert!(!png.is_empty());
        let img = image::load_from_memory(&png).unwrap();
        use image::GenericImageView;
        assert_eq!(img.dimensions(), (128, 128));
    }

    #[test]
    fn renders_mr_small_to_valid_png() {
        let bytes = sample("MR_small.dcm");
        let png = render_png(&bytes).unwrap();
        assert!(!png.is_empty());
        let img = image::load_from_memory(&png).unwrap();
        use image::GenericImageView;
        assert_eq!(img.dimensions(), (64, 64));
    }

    #[test]
    fn parse_study_date_rejects_malformed_input() {
        assert_eq!(
            parse_study_date("20040119"),
            Some("2004-01-19T00:00:00+00:00".to_string())
        );
        assert_eq!(parse_study_date(""), None);
        assert_eq!(parse_study_date("2004-01-19"), None);
        assert_eq!(parse_study_date("99999999"), None);
    }

    #[test]
    fn parse_meta_errors_on_garbage_bytes() {
        assert!(parse_meta(b"not a dicom file").is_err());
    }

    /// Builds a minimal Explicit-VR-LE DICOM whose header declares `rows`x`cols`
    /// (16-bit grayscale) but carries only a few bytes of actual PixelData — the
    /// shape of a "tiny file, huge declared dims" allocation/decompression bomb.
    fn oversized_dcm(rows: u16, cols: u16) -> Vec<u8> {
        use dicom_core::{dicom_value, DataElement, PrimitiveValue, VR};
        use dicom_dictionary_std::tags;
        use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(
                tags::PHOTOMETRIC_INTERPRETATION,
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15_u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0_u16)),
            // Deliberately tiny actual pixel data — far smaller than the header
            // claims. A decoder that trusted the header would over-allocate.
            DataElement::new(tags::PIXEL_DATA, VR::OW, dicom_value!(U16, [0, 0, 0, 0])),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1") // Explicit VR Little Endian
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.7")
            .media_storage_sop_instance_uid("1.2.3.4.5")
            .build()
            .expect("build file meta");
        let mut out = Vec::new();
        obj.with_exact_meta(meta)
            .write_all(&mut out)
            .expect("write DICOM");
        out
    }

    #[test]
    fn render_png_rejects_oversized_dimensions_before_alloc() {
        // 60000 x 60000 x 2 bytes = ~6.7 GB declared — far over the 512 MiB cap.
        let bytes = oversized_dcm(60000, 60000);
        let err = render_png(&bytes).expect_err("oversized dims must be rejected");
        assert!(
            err.to_string().contains("decode cap"),
            "expected the size-cap guard to fire, got: {err:#}"
        );
    }

    #[test]
    fn decode_frame_rejects_oversized_dimensions_before_alloc() {
        let bytes = oversized_dcm(60000, 60000);
        let err = decode_frame(&bytes, 0).expect_err("oversized dims must be rejected");
        assert!(
            err.to_string().contains("decode cap"),
            "expected the size-cap guard to fire, got: {err:#}"
        );
    }

    #[test]
    fn sane_dimensions_pass_the_bounds_check() {
        // A normally-sized instance sails through the cap and decodes as before.
        let bytes = sample("CT_small.dcm");
        assert!(render_png(&bytes).is_ok());
        assert!(decode_frame(&bytes, 0).is_ok());
    }

    #[test]
    fn decodes_uncompressed_ct_frame() {
        let bytes = sample("CT_small.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (128, 128));
        assert_eq!(frame.header.samples_per_pixel, 1);
        assert_eq!(frame.header.bits_allocated, 16);
        assert_eq!(frame.header.pixel_representation, 1); // CT_small is signed
                                                          // 16-bit grayscale → 2 bytes/sample.
        assert_eq!(frame.pixels.len(), 128 * 128 * 2);
    }

    #[test]
    fn decodes_jpeg2000_frame() {
        // JPEG2000.dcm: TS 1.2.840.10008.1.2.4.91, 1024x256 grayscale.
        let bytes = sample("JPEG2000.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (1024, 256));
        assert_eq!(frame.header.samples_per_pixel, 1);
        assert!(!frame.pixels.is_empty());
        assert_eq!(
            frame.pixels.len(),
            1024 * 256 * (frame.header.bits_allocated as usize / 8)
        );
    }

    #[test]
    fn decodes_jpeg2000_lossless_grayscale_512() {
        // 693_J2KR.dcm: TS ...4.90 (JPEG2000 lossless), 512x512 16-bit grayscale
        // — a real-world image the pure-Rust openjp2 backend crashed on, which is
        // why we decode J2K via vendored OpenJPEG (openjpeg-sys).
        let bytes = sample("693_J2KR.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (512, 512));
        assert_eq!(frame.header.samples_per_pixel, 1);
        assert_eq!(
            frame.pixels.len(),
            512 * 512 * (frame.header.bits_allocated as usize / 8)
        );
    }

    #[test]
    fn decodes_jpeg2000_color_ultrasound() {
        // US1_J2KR.dcm: TS ...4.90 (JPEG2000 lossless), 480x640 color.
        let bytes = sample("US1_J2KR.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (480, 640));
        assert_eq!(frame.header.samples_per_pixel, 3);
        assert_eq!(frame.header.photometric, "RGB");
        assert_eq!(frame.pixels.len(), 480 * 640 * 3);
    }

    #[test]
    fn decodes_jpeg_ls_lossless_frame() {
        // MR_small_jpeg_ls_lossless.dcm: TS ...4.80, 64x64 grayscale.
        let bytes = sample("MR_small_jpeg_ls_lossless.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (64, 64));
        assert_eq!(frame.header.samples_per_pixel, 1);
        assert!(!frame.pixels.is_empty());
    }

    #[test]
    fn decodes_rle_lossless_frame() {
        // MR_small_RLE.dcm: TS 1.2.840.10008.1.2.5, 64x64 grayscale.
        let bytes = sample("MR_small_RLE.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        assert_eq!((frame.header.rows, frame.header.cols), (64, 64));
        assert_eq!(frame.header.samples_per_pixel, 1);
        assert!(!frame.pixels.is_empty());
    }

    #[test]
    fn ipc_bytes_roundtrip_header_and_pixels() {
        let bytes = sample("MR_small_RLE.dcm");
        let frame = decode_frame(&bytes, 0).unwrap();
        let header = frame.header.clone();
        let pixel_len = frame.pixels.len();
        let wire = frame.into_ipc_bytes().unwrap();
        let hlen = u32::from_le_bytes(wire[0..4].try_into().unwrap()) as usize;
        let parsed: DecodedFrameHeader = serde_json::from_slice(&wire[4..4 + hlen]).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(wire.len() - 4 - hlen, pixel_len);
    }
}
